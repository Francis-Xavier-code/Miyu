//! WebUI 命令平面。

use super::shared::*;
use crate::web::*;

/// `GET /api/commands` 的返回形状是前端的契约：`commands.js` 拿 `name` 做精确
/// 匹配、拿 `arg_hint` 判断收不收参数、拿 `help` 渲染菜单第二列。任何一项缺了
/// 菜单就渲染不出来，而那是纯前端行为，Rust 侧看不见。
#[tokio::test]
async fn command_catalog_carries_what_the_menu_needs() {
    let temp = tempfile::tempdir().unwrap();
    let state = DaemonState::for_test(test_paths(temp.path()), 8300).unwrap();

    let response = list_commands(axum::extract::State(state), HeaderMap::new())
        .await
        .expect("未设密码时不该要鉴权");
    let commands = response.0["commands"].as_array().unwrap().clone();

    assert!(!commands.is_empty(), "一条命令都没返回，前端菜单是空的");
    for command in &commands {
        let name = command["name"].as_str().expect("缺 name");
        assert!(name.starts_with('/'), "{name} 不是斜杠命令");
        assert!(
            command["arg_hint"].is_string(),
            "{name} 缺 arg_hint——前端据它判断收不收参数"
        );
        assert!(
            command["help"]
                .as_str()
                .is_some_and(|help| !help.is_empty()),
            "{name} 缺帮助文案"
        );
    }

    // 与 `web_commands()` 同源：这里返回的必须正好是打了 web 标记的那批。
    let expected = crate::slash_commands::web_commands()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    let actual = commands
        .iter()
        .map(|command| command["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

/// `/reset-memory` 走 WebUI 时必须真的清掉那份记忆，且 dev 与普通模式各清各的
/// ——dev 的记忆挂在保留人格名下，钥匙不对就清的是另一份（与
/// `IpcCommand::ResetMemory` 同一个坑）。
#[tokio::test]
async fn web_memory_reset_clears_the_mode_it_was_asked_for() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let state = DaemonState::for_test(paths.clone(), 8301).unwrap();
    let config = state.manager.lock().unwrap().config.clone();

    let normal = crate::memory::MemoryStore::new(&config, &paths);
    let dev = crate::memory::MemoryStore::new(&config.dev_scoped(), &paths);
    normal
        .remember_fact("普通模式记住 XMODIFIERS 这件事", "test")
        .unwrap();
    dev.remember_fact("开发模式记住 XMODIFIERS 这件事", "test")
        .unwrap();
    let recalled = |store: &crate::memory::MemoryStore| {
        store
            .recall_memories("XMODIFIERS", 5, false)
            .unwrap()
            .to_string()
    };
    assert!(recalled(&normal).contains("普通模式"));
    assert!(recalled(&dev).contains("开发模式"));

    reset_memory_http(
        axum::extract::State(state.clone()),
        HeaderMap::new(),
        axum::Json(serde_json::from_value(serde_json::json!({ "mode": "dev" })).unwrap()),
    )
    .await
    .unwrap();

    assert!(
        !recalled(&dev).contains("开发模式"),
        "点名清 dev，dev 的记忆却还在"
    );
    assert!(
        recalled(&normal).contains("普通模式"),
        "只该清 dev，普通模式的记忆被误伤了"
    );
}
