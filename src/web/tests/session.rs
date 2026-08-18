//! 会话切换、重置、人格与模型覆盖。

use super::shared::*;
use crate::runtime::LOGIN_ATTEMPT_LIMIT;
use crate::state::PlatformSessionBindingKey;
use crate::web::*;

#[test]
fn managed_persona_assets_use_the_resource_directory_and_reject_escape() {
    let temp = tempfile::tempdir().unwrap();
    let mut paths = test_paths(temp.path());
    paths.skills_dir = paths.data_dir.join("skills");
    paths.scripts_dir = paths.data_dir.join("scripts");

    assert_eq!(
        managed_persona_asset_path(&paths, "persona-avatars/avatar.png"),
        Some(paths.data_dir.join("persona-avatars/avatar.png"))
    );
    assert!(managed_persona_asset_path(&paths, "/etc/passwd").is_none());
    assert!(managed_persona_asset_path(&paths, "persona-avatars/../secret").is_none());
    assert_eq!(
        managed_persona_asset_path(&paths, "persona-avatars/nested/file.png"),
        Some(paths.data_dir.join("persona-avatars/nested/file.png"))
    );
    assert_eq!(
        resolve_persona_asset_path(&paths, "./persona-avatars/avatar.png"),
        Some(paths.data_dir.join("persona-avatars/avatar.png"))
    );
    assert!(resolve_persona_asset_path(&paths, "persona-avatars/../../secret").is_none());
    assert_eq!(
        resolve_persona_asset_path(&paths, "avatars/custom.png"),
        Some(paths.config_dir.join("avatars/custom.png"))
    );
    assert_eq!(
        resolve_persona_asset_path(&paths, "scripts/images/custom.png"),
        Some(paths.data_dir.join("scripts/images/custom.png"))
    );
    assert_eq!(
        resolve_persona_asset_path(
            &paths,
            &paths
                .config_dir
                .join("persona-avatars/absolute.png")
                .display()
                .to_string(),
        ),
        Some(paths.data_dir.join("persona-avatars/absolute.png"))
    );
}

#[test]
fn persona_asset_cleanup_normalizes_managed_reference_paths() {
    fn prompts(path: String) -> PromptDocuments {
        PromptDocuments {
            personas: vec![PromptDocument {
                name: "Persona.md".to_string(),
                content: String::new(),
                avatar_path: Some(path),
                board_image_path: None,
                board_title: None,
                board_subtitle: None,
                starter_prompts: None,
                original_name: None,
            }],
            identities: Vec::new(),
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut paths = test_paths(temp.path());
    paths.skills_dir = paths.data_dir.join("skills");
    let directory = paths.persona_avatars_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let name = format!("{}.png", "a".repeat(64));
    let asset = directory.join(&name);
    std::fs::write(&asset, "image").unwrap();

    cleanup_persona_assets(
        &paths,
        &prompts(format!("persona-avatars/{name}")),
        &prompts(format!("./persona-avatars/{name}")),
    );
    assert!(asset.is_file());
}

#[cfg(unix)]
#[test]
fn managed_persona_asset_validation_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let mut paths = test_paths(temp.path());
    paths.skills_dir = paths.data_dir.join("skills");
    let directory = paths.persona_avatars_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let outside = temp.path().join("outside.png");
    std::fs::write(&outside, "image").unwrap();
    let managed = directory.join("avatar.png");
    symlink(&outside, &managed).unwrap();

    assert!(validate_managed_persona_asset_file(&paths, &managed).is_err());
}

#[test]
fn target_session_state_does_not_move_the_default_session() {
    let temp = tempfile::tempdir().unwrap();
    let state = DaemonState::for_test(test_paths(temp.path()), 8300).unwrap();
    let persona = active_persona_scope(&state);
    state
        .state_store
        .adopt_sessions_for_persona(&persona)
        .unwrap();
    let default_session_id = state.state_store.session_id();
    let local = state
        .state_store
        .create_session(&persona, "repl local", "user", None)
        .unwrap();

    let snapshot = session_state_for(&state, &local.session_id).unwrap();

    assert_eq!(snapshot.session_id, local.session_id);
    assert_eq!(&*state.state_store.session_id(), &*default_session_id);
}

#[test]
fn local_session_resolution_rejects_platform_ids_and_prefers_local_names() {
    let temp = tempfile::tempdir().unwrap();
    let state = DaemonState::for_test(test_paths(temp.path()), 8300).unwrap();
    let persona = active_persona_scope(&state);
    state
        .state_store
        .adopt_sessions_for_persona(&persona)
        .unwrap();
    let local = state
        .state_store
        .create_session(&persona, "shared", "user", None)
        .unwrap();
    let platform = state
        .state_store
        .create_session(&persona, "shared", "user", None)
        .unwrap();
    state
        .state_store
        .bind_platform_session(
            &PlatformSessionBindingKey {
                platform: "onebot".to_string(),
                account_id: "10000".to_string(),
                conversation_kind: "private".to_string(),
                conversation_id: "20000".to_string(),
                participant_id: Some("20000".to_string()),
                persona,
            },
            &platform.session_id,
        )
        .unwrap();

    let resolved = resolve_local_session_ref(
        &state,
        &ipc::SessionRef::Name {
            name: "SHARED".to_string(),
        },
    )
    .unwrap();
    assert_eq!(resolved.session_id, local.session_id);
    assert!(resolve_local_session_ref(
        &state,
        &ipc::SessionRef::Id {
            id: platform.session_id,
        },
    )
    .is_err());
}

#[test]
fn startup_repairs_a_platform_owned_current_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = StateStore::new(&test_paths(temp.path())).unwrap();
    store.adopt_sessions_for_persona("miyu").unwrap();
    let qq_session = store
        .create_session("miyu", "QQ group 20000", "user", None)
        .unwrap();
    store
        .bind_platform_session(
            &PlatformSessionBindingKey {
                platform: "onebot".to_string(),
                account_id: "10000".to_string(),
                conversation_kind: "group".to_string(),
                conversation_id: "20000".to_string(),
                participant_id: None,
                persona: "miyu".to_string(),
            },
            &qq_session.session_id,
        )
        .unwrap();
    store.switch_session(&qq_session.session_id).unwrap();

    ensure_local_current_session(&store, "miyu").unwrap();

    let repaired = store.session_id();
    assert_ne!(&*repaired, qq_session.session_id);
    assert!(!store.is_platform_session(&repaired).unwrap());
    assert_eq!(
        store.session_record(&repaired).unwrap().unwrap().persona,
        "miyu"
    );
}

#[test]
fn persona_file_mutations_include_avatar_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let mut mutations = HashMap::new();
    let documents = vec![PromptDocument {
        name: "Alice.md".to_string(),
        content: "prompt".to_string(),
        avatar_path: Some("avatars/alice.png".to_string()),
        board_image_path: None,
        board_title: None,
        board_subtitle: None,
        starter_prompts: None,
        original_name: None,
    }];
    collect_prompt_file_mutations(
        &[],
        &documents,
        temp.path(),
        temp.path(),
        &mut mutations,
        true,
    );

    let metadata = mutations
        .get(&temp.path().join("Alice.json"))
        .and_then(Option::as_deref)
        .unwrap();
    let metadata: Value = serde_json::from_slice(metadata).unwrap();
    assert_eq!(metadata["avatar_path"], "avatars/alice.png");
}

#[test]
fn persona_identity_uses_default_and_custom_values() {
    let mut config = AppConfig::default();
    let prompts = PromptDocuments::default();
    let default = persona_identity(&config, &prompts);
    assert_eq!(default.name, "Miyu");
    assert_eq!(default.avatar_url.as_deref(), Some("/assets/miyu-logo.png"));

    config.prompt.active_persona = "Alice.md".to_string();
    let prompts = PromptDocuments {
        personas: vec![PromptDocument {
            name: "Alice.md".to_string(),
            content: "prompt".to_string(),
            avatar_path: Some("avatars/alice.png".to_string()),
            board_image_path: None,
            board_title: None,
            board_subtitle: None,
            starter_prompts: None,
            original_name: None,
        }],
        identities: Vec::new(),
    };
    let custom = persona_identity(&config, &prompts);
    assert_eq!(custom.name, "Alice");
    assert_eq!(custom.avatar_url.as_deref(), Some("/api/persona/avatar"));
}

#[test]
fn sanitize_session_title_cleans_llm_output() {
    assert_eq!(sanitize_session_title("「东京天气查询」"), "东京天气查询");
    assert_eq!(
        sanitize_session_title("\"Arch Linux 新闻\"\n第二行忽略"),
        "Arch Linux 新闻"
    );
    assert_eq!(sanitize_session_title("  标题。  "), "标题");
    assert_eq!(sanitize_session_title(""), "");
    // Overlong output clips to 20 chars.
    let long = "很长的标题".repeat(10);
    assert_eq!(sanitize_session_title(&long).chars().count(), 20);
}

#[test]
fn optional_password_auth_issues_server_side_sessions_and_limits_failures() {
    let disabled = WebAuth::new(None);
    assert!(disabled.is_authenticated(None));

    let auth = WebAuth::new(Some("correct horse"));
    let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);
    assert!(!auth.is_authenticated(None));
    assert!(matches!(
        auth.login(peer, "wrong"),
        Err(LoginFailure::Invalid)
    ));
    let token = auth.login(peer, "correct horse").unwrap();
    assert!(auth.is_authenticated(Some(&token)));

    let limited = WebAuth::new(Some("secret"));
    for _ in 0..LOGIN_ATTEMPT_LIMIT {
        assert!(matches!(
            limited.login(peer, "wrong"),
            Err(LoginFailure::Invalid)
        ));
    }
    assert!(matches!(
        limited.login(peer, "secret"),
        Err(LoginFailure::RateLimited)
    ));
}

#[test]
fn model_selection_rejects_empty_and_duplicate_pools() {
    assert!(validate_model_selection(Vec::new()).is_err());
    let model = ActiveProviderModelConfig {
        provider_id: "provider".to_string(),
        model: "model".to_string(),
    };
    assert!(validate_model_selection(vec![model.clone()]).is_ok());
    assert!(validate_model_selection(vec![model.clone(), model]).is_err());
}

#[test]
fn thinking_variant_validation_distinguishes_model_default_and_named_default() {
    let updates = validate_thinking_variant_updates(vec![
        ThinkingVariantUpdate {
            provider_id: " provider ".to_string(),
            model: "model-one".to_string(),
            selected: None,
        },
        ThinkingVariantUpdate {
            provider_id: "provider".to_string(),
            model: "model-two".to_string(),
            selected: Some(" default ".to_string()),
        },
    ])
    .unwrap();
    assert_eq!(updates[0].provider_id, "provider");
    assert_eq!(updates[0].selected, None);
    assert_eq!(updates[1].selected.as_deref(), Some("default"));

    assert!(validate_thinking_variant_updates(vec![
        ThinkingVariantUpdate {
            provider_id: "provider".to_string(),
            model: "model".to_string(),
            selected: None,
        },
        ThinkingVariantUpdate {
            provider_id: " provider ".to_string(),
            model: " model ".to_string(),
            selected: Some("high".to_string()),
        },
    ])
    .is_err());
}

#[test]
fn thinking_variant_updates_validate_before_persisting_and_can_clear_a_selection() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let config = AppConfig::default();
    let choice = config
        .active_provider_model_choices()
        .into_iter()
        .next()
        .unwrap();
    let mut preferences = ThinkingVariantPreferences::load(&paths);
    preferences.set(
        &choice.provider_id,
        &choice.model,
        Some("previous-selection".to_string()),
    );
    preferences.save(&paths).unwrap();

    let mut agent = None;
    let invalid = ThinkingVariantUpdate {
        provider_id: choice.provider_id.clone(),
        model: choice.model.clone(),
        selected: Some("definitely-not-a-real-variant".to_string()),
    };
    assert!(matches!(
        apply_thinking_variant_updates(&mut agent, &config, &paths, &[invalid]),
        Err(AdminFailure::Invalid(_))
    ));
    assert_eq!(
        ThinkingVariantPreferences::load(&paths).selected(&choice.provider_id, &choice.model),
        Some("previous-selection")
    );

    let clear = ThinkingVariantUpdate {
        provider_id: choice.provider_id.clone(),
        model: choice.model.clone(),
        selected: None,
    };
    apply_thinking_variant_updates(&mut agent, &config, &paths, &[clear]).unwrap();
    assert_eq!(
        ThinkingVariantPreferences::load(&paths).selected(&choice.provider_id, &choice.model),
        None
    );
}

#[test]
fn web_persona_rename_updates_qq_routes_and_deletion_is_rejected() {
    let mut config = AppConfig::default();
    config
        .platforms
        .qq
        .conversations
        .push(crate::config::PlatformModelRoute {
            conversation: crate::config::PlatformConversationConfig {
                kind: crate::config::PlatformConversationKind::Group,
                id: "42".to_string(),
            },
            persona: crate::config::PlatformPersonaOverride::Custom {
                name: "Old.md".to_string(),
            },
            text_models_inheritance: crate::config::PlatformModelPoolInheritance::Platform,
            text_models: None,
            multimodal_models_inheritance: crate::config::PlatformModelPoolInheritance::Platform,
            multimodal_models: None,
            extra_prompt: String::new(),
            session_limits: None,
        });
    let renamed: PromptDocuments = serde_json::from_value(json!({
        "personas": [{
            "name": "New.md",
            "content": "persona",
            "original_name": "Old.md"
        }],
        "identities": []
    }))
    .unwrap();

    reconcile_qq_persona_references(&mut config, &renamed);
    assert_eq!(
        config.platforms.qq.conversations[0].persona.custom_name(),
        Some("New.md")
    );
    assert!(validate_prompt_documents(&config, &renamed).is_ok());
    assert!(validate_prompt_documents(&config, &PromptDocuments::default()).is_err());
}

#[test]
fn web_persona_renames_use_the_original_reference_snapshot() {
    let route = |id: &str, persona: &str| crate::config::PlatformModelRoute {
        conversation: crate::config::PlatformConversationConfig {
            kind: crate::config::PlatformConversationKind::Group,
            id: id.to_string(),
        },
        persona: crate::config::PlatformPersonaOverride::Custom {
            name: persona.to_string(),
        },
        text_models_inheritance: crate::config::PlatformModelPoolInheritance::Platform,
        text_models: None,
        multimodal_models_inheritance: crate::config::PlatformModelPoolInheritance::Platform,
        multimodal_models: None,
        extra_prompt: String::new(),
        session_limits: None,
    };
    let mut config = AppConfig::default();
    config.platforms.qq.conversations = vec![route("1", "A.md"), route("2", "B.md")];
    let prompts: PromptDocuments = serde_json::from_value(json!({
        "personas": [
            {"name": "B.md", "content": "A", "original_name": "A.md"},
            {"name": "C.md", "content": "B", "original_name": "B.md"}
        ],
        "identities": []
    }))
    .unwrap();

    reconcile_qq_persona_references(&mut config, &prompts);

    assert_eq!(
        config.platforms.qq.conversations[0].persona.custom_name(),
        Some("B.md")
    );
    assert_eq!(
        config.platforms.qq.conversations[1].persona.custom_name(),
        Some("C.md")
    );
}

#[test]
fn web_rejects_persona_names_with_colliding_persistent_scopes() {
    let prompts: PromptDocuments = serde_json::from_value(json!({
        "personas": [
            {"name": "A B.md", "content": "first"},
            {"name": "A@B.md", "content": "second"}
        ],
        "identities": []
    }))
    .unwrap();

    assert!(validate_prompt_documents(&AppConfig::default(), &prompts).is_err());
}

#[test]
fn web_persona_scope_batch_migration_supports_swaps() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let store = StateStore::new(&paths).unwrap();
    let first = store.create_session("a", "first", "user", None).unwrap();
    let second = store.create_session("b", "second", "user", None).unwrap();

    migrate_persona_db_scopes(
        &store,
        &[
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ],
    )
    .unwrap();

    assert_eq!(
        store
            .session_record(&first.session_id)
            .unwrap()
            .unwrap()
            .persona,
        "b"
    );
    assert_eq!(
        store
            .session_record(&second.session_id)
            .unwrap()
            .unwrap()
            .persona,
        "a"
    );
}
