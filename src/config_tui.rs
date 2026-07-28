use crate::config::{
    ActiveProviderModelConfig, AppConfig, PlatformConversationConfig, PlatformConversationKind,
    PlatformModelRoute, ProviderConfig, MAX_COMMAND_OUTPUT_LINES,
};
use crate::default_kb;
use crate::default_models::{OPENCODE_DEFAULT_VISION_MODEL, OPENCODE_PROVIDER_ID};
use crate::i18n::{is_zh, text as t};
use crate::paths::MiyuPaths;
use anyhow::{bail, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub fn run(paths: &MiyuPaths) -> Result<bool> {
    AppConfig::init_files(paths)?;
    crate::models_cache::try_load(paths);
    crate::models_cache::spawn_background_refresh(paths.clone());
    let config = AppConfig::load_or_default(paths)?;
    TerminalSession::start()?.run(paths, config)
}

struct TerminalSession {
    stdout: io::Stdout,
}

impl TerminalSession {
    fn start() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide)?;
        Ok(Self { stdout })
    }

    fn run(mut self, paths: &MiyuPaths, mut config: AppConfig) -> Result<bool> {
        let result = run_main_menu(&mut self.stdout, paths, &mut config);
        execute!(self.stdout, Show, LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        result
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn run_main_menu(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &mut AppConfig,
) -> Result<bool> {
    let mut selected = 0usize;
    loop {
        let active = active_label(config);
        let multimodal = active_multimodal_label(config);
        let options = [
            format!(
                "{} ({}: {active})",
                t("Configure text model", "配置文本模型"),
                t("Current", "当前")
            ),
            format!(
                "{} ({}: {multimodal})",
                t("Configure multimodal model", "配置多模态模型"),
                t("Current", "当前")
            ),
            format!(
                "{} ({})",
                t("Configure subagent tier pools", "配置子代理档位池"),
                subagent_tiers_label(config)
            ),
            t("Providers and models", "供应商和模型").to_string(),
            t("Plugins", "插件配置").to_string(),
            t("Custom prompts", "自定义提示词").to_string(),
            format!(
                "{} ({})",
                t("IM platforms", "接入通讯平台"),
                platforms_label(config)
            ),
            t("Global settings", "全局参数设置").to_string(),
            t("Save and exit", "保存并退出").to_string(),
        ];
        let status = default_kb::status(paths)
            .ok()
            .filter(|status| status.has_update_notice)
            .map(|_| {
                t(
                    "The default knowledge base needs an update; run miyu update-default-kb",
                    "默认知识库需要更新，运行 miyu update-default-kb",
                )
            })
            .unwrap_or("");
        draw_menu(
            stdout,
            t(" MIYU CONFIG ", " MIYU 配置 "),
            &options,
            selected,
            status,
        )?;

        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => match selected {
                0 => select_active_provider(stdout, config)?,
                1 => select_active_multimodal_provider(stdout, config)?,
                2 => select_subagent_tiers(stdout, config)?,
                3 => ProviderBrowser::new(paths, config).run(stdout)?,
                4 => edit_plugins(stdout, config)?,
                5 => edit_custom_prompts(stdout, paths, config)?,
                6 => select_platforms(stdout, config)?,
                7 => edit_settings(stdout, config)?,
                8 => {
                    config.save(paths)?;
                    return Ok(true);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn edit_plugins(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let count = plugin_names().len();
        draw_plugin_menu(stdout, config, selected)?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(count - 1),
            KeyCode::Char(' ') => toggle_plugin(config, selected),
            KeyCode::Enter | KeyCode::Char('i') => edit_plugin_detail(stdout, config, selected)?,
            _ => {}
        }
    }
}

fn draw_plugin_menu(stdout: &mut io::Stdout, config: &AppConfig, selected: usize) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let width = cols.saturating_sub(4).max(60);
    let height = rows.saturating_sub(2).max(10);
    let x = 2;
    let y = 1;
    queue!(stdout, Clear(ClearType::All))?;
    draw_box(stdout, x, y, width, height, t(" PLUGINS ", " 插件 "))?;
    queue!(
        stdout,
        MoveTo(x + 2, y + 1),
        Print(t(
            "[Space]enable/disable [Enter]configure [j/k]move [q]back",
            "[Space]启用/禁用 [Enter]配置 [j/k]移动 [q]返回",
        ))
    )?;
    queue!(
        stdout,
        MoveTo(x + 2, y + 3),
        SetAttribute(Attribute::Bold),
        Print(pad(
            &plugin_row(
                t("Status", "状态"),
                t("Plugin", "插件"),
                t("Description", "说明"),
                width.saturating_sub(4) as usize,
            ),
            width.saturating_sub(4) as usize,
        )),
        SetAttribute(Attribute::Reset)
    )?;
    let plugins = plugin_names();
    let visible_rows = height.saturating_sub(6) as usize;
    let start = selected.saturating_sub(visible_rows.saturating_sub(1));
    for row in 0..visible_rows {
        let index = start + row;
        if index >= plugins.len() {
            break;
        }
        let (_, name, description) = plugins[index];
        let state = if plugin_enabled(config, index) {
            t("[ON]", "[开]")
        } else {
            t("[OFF]", "[关]")
        };
        let line = plugin_row(state, name, description, width.saturating_sub(4) as usize);
        queue!(stdout, MoveTo(x + 2, y + row as u16 + 4))?;
        if index == selected {
            queue!(
                stdout,
                SetAttribute(Attribute::Reverse),
                Print(pad(&line, width.saturating_sub(4) as usize)),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(stdout, Print(pad(&line, width.saturating_sub(4) as usize)))?;
        }
    }
    stdout.flush()?;
    Ok(())
}

fn plugin_row(state: &str, name: &str, description: &str, width: usize) -> String {
    let fixed = pad(state, 8) + &pad(name, 24);
    let remaining = width.saturating_sub(display_width(&fixed)).max(10);
    fixed + &truncate(description, remaining)
}

fn plugin_names() -> [(&'static str, &'static str, &'static str); 13] {
    [
        (
            "web",
            t("Web search", "网络搜索"),
            t(
                "Search APIs with script fallback",
                "搜索 API 与脚本 fallback",
            ),
        ),
        (
            "deep_research",
            t("Deep research", "深度研究"),
            t(
                "Run long research tasks and output Markdown",
                "长任务研究并输出 Markdown",
            ),
        ),
        (
            "vision",
            t("Vision", "识图"),
            t(
                "Image understanding and terminal preview",
                "图片理解和终端预览",
            ),
        ),
        (
            "image_generation",
            t("Image generation", "生图"),
            t("Generate images from text", "文本生成图片"),
        ),
        (
            "web_images",
            t("Image search", "搜图"),
            t(
                "Search, download, and review web images",
                "网络图片搜索、下载与审核",
            ),
        ),
        (
            "print_image",
            t("Print image", "打印图片"),
            t("Terminal image print size", "终端图片打印尺寸"),
        ),
        (
            "memes",
            t("Memes", "表情包"),
            t("Persona meme library and send size", "人格表情库与发送尺寸"),
        ),
        (
            "knowledge_base",
            t("Knowledge base", "知识库"),
            t(
                "Local file search and semantic index",
                "本地文件检索与语义索引",
            ),
        ),
        (
            "archlinux",
            "Arch Linux",
            t("AUR status and ArchWiki lookup", "AUR 状态与 ArchWiki 查询"),
        ),
        (
            "man",
            t("Online manuals", "在线手册"),
            t(
                "Search and read online man pages",
                "在线 man 手册搜索与读取",
            ),
        ),
        (
            "memory",
            t("Memory", "记忆"),
            t("Long-term memory and association", "长期记忆与联想"),
        ),
        (
            "package_advisor",
            t("AUR review", "AUR 审查"),
            t("PKGBUILD/AUR security review", "PKGBUILD/AUR 安全审查"),
        ),
        (
            "deep_research_linux_game_compatibility",
            t("Linux game compatibility", "Linux 游戏兼容"),
            t(
                "Proton/anti-cheat/compatibility lookup",
                "Proton/反作弊/兼容性查询",
            ),
        ),
    ]
}

fn plugin_enabled(config: &AppConfig, index: usize) -> bool {
    match index {
        0 => config.plugins.web.enabled,
        1 => config.plugins.deep_research.enabled,
        2 => config.plugins.vision.enabled,
        3 => config.plugins.image_generation.enabled,
        4 => config.plugins.web_images.enabled,
        5 => config.plugins.print_image.enabled,
        6 => config.plugins.memes.enabled,
        7 => config.plugins.knowledge_base.enabled,
        8 => config.plugins.archlinux.enabled,
        9 => config.plugins.man.enabled,
        10 => config.plugins.memory.enabled,
        11 => config.plugins.package_advisor.enabled,
        12 => {
            config
                .plugins
                .deep_research_linux_game_compatibility
                .enabled
        }
        _ => false,
    }
}

fn toggle_plugin(config: &mut AppConfig, index: usize) {
    let value = !plugin_enabled(config, index);
    match index {
        0 => config.plugins.web.enabled = value,
        1 => config.plugins.deep_research.enabled = value,
        2 => config.plugins.vision.enabled = value,
        3 => config.plugins.image_generation.enabled = value,
        4 => config.plugins.web_images.enabled = value,
        5 => config.plugins.print_image.enabled = value,
        6 => config.plugins.memes.enabled = value,
        7 => config.plugins.knowledge_base.enabled = value,
        8 => config.plugins.archlinux.enabled = value,
        9 => config.plugins.man.enabled = value,
        10 => config.plugins.memory.enabled = value,
        11 => config.plugins.package_advisor.enabled = value,
        12 => {
            config
                .plugins
                .deep_research_linux_game_compatibility
                .enabled = value
        }
        _ => {}
    }
}

fn edit_plugin_detail(stdout: &mut io::Stdout, config: &mut AppConfig, index: usize) -> Result<()> {
    let title = format!(" {}: {} ", t("PLUGIN", "插件"), plugin_names()[index].1);
    let mut fields = plugin_fields(config, index);
    if !run_form(stdout, &title, &mut fields)? {
        return Ok(());
    }
    apply_plugin_fields(config, index, &fields)
}

fn plugin_fields(config: &AppConfig, index: usize) -> Vec<Field> {
    match index {
        0 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.web.enabled),
            Field::new(
                t("Results per request", "每次返回数量"),
                config.plugins.web.max_results.to_string(),
            ),
            Field::textarea(
                "Tavily API Keys",
                config.plugins.web.tavily_api_keys.join("\n"),
            )
            .sensitive(),
            Field::textarea(
                "Firecrawl API Keys",
                config.plugins.web.firecrawl_api_keys.join("\n"),
            )
            .sensitive(),
            Field::textarea(
                "AnySearch API Keys",
                config.plugins.web.anysearch_api_keys.join("\n"),
            )
            .sensitive(),
            Field::new("SearXNG URL", config.plugins.web.searxng_base_url.clone()),
        ],
        1 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.deep_research.enabled),
            Field::new(
                t("Output directory", "输出目录"),
                config.plugins.deep_research.output_dir.clone(),
            ),
            Field::new(
                t("Thinking depth", "思考深度"),
                config.plugins.deep_research.thinking_depth.clone(),
            )
            .choices(&["minimal", "low", "medium", "high", "xhigh"]),
            Field::new(
                t("Maximum review revisions", "最大审视修正次数"),
                config
                    .plugins
                    .deep_research
                    .max_review_revisions
                    .to_string(),
            ),
            Field::new(
                t("Tool steps per round", "每轮工具步数"),
                config
                    .plugins
                    .deep_research
                    .max_tool_steps_per_round
                    .to_string(),
            ),
            Field::new(
                t("Final answer character limit", "最终字数上限"),
                config
                    .plugins
                    .deep_research
                    .max_final_answer_chars
                    .to_string(),
            ),
            Field::new(
                t("Tool timeout (seconds)", "工具超时秒数"),
                config
                    .plugins
                    .deep_research
                    .tool_call_timeout_seconds
                    .to_string(),
            ),
            Field::boolean(
                t("Show progress", "显示过程进度"),
                config.plugins.deep_research.show_progress,
            ),
        ],
        2 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.vision.enabled),
            Field::boolean(
                t(
                    "Prefer current chat model for images",
                    "优先使用当前对话模型识图",
                ),
                config.plugins.vision.prefer_current_multimodal_model,
            ),
            Field::new(
                t("Vision provider/model", "识图 Provider/模型"),
                vision_provider_value(config),
            )
            .choices_owned(vision_provider_model_choice_values(config)),
        ],
        3 => vec![
            Field::boolean(
                t("Enabled", "启用"),
                config.plugins.image_generation.enabled,
            ),
            Field::new(
                t("Image API type", "生图 API 类型"),
                config.plugins.image_generation.provider_type.clone(),
            )
            .choices(&["openai", "rightcode"]),
            Field::new("Base URL", config.plugins.image_generation.base_url.clone()),
            Field::textarea(
                "API Keys",
                config.plugins.image_generation.api_keys.join("\n"),
            )
            .sensitive(),
            Field::new(
                t("Model", "模型"),
                config.plugins.image_generation.model.clone(),
            ),
            Field::new(
                t("Default aspect ratio", "默认宽高比"),
                config.plugins.image_generation.default_aspect_ratio.clone(),
            )
            .choices(&[
                "自动", "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9",
            ]),
            Field::new(
                t("Default resolution", "默认分辨率"),
                config.plugins.image_generation.default_resolution.clone(),
            )
            .choices(&["1K", "2K", "4K"]),
            Field::new(
                t("Output directory", "输出目录"),
                config.plugins.image_generation.output_dir.clone(),
            ),
            Field::boolean(
                t("Print when complete", "完成后打印"),
                config.plugins.image_generation.auto_print,
            ),
            Field::new(
                t("Timeout (seconds)", "超时秒数"),
                config.plugins.image_generation.timeout_seconds.to_string(),
            ),
        ],
        4 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.web_images.enabled),
            Field::new(
                t("Search source mode", "搜索来源模式"),
                config.plugins.web_images.source_mode.clone(),
            )
            .choices(&["auto", "global", "mainland"]),
            Field::boolean(
                t("Vision model review", "视觉模型审核"),
                config.plugins.web_images.vision_screening_enabled,
            ),
            Field::new(
                t("Maximum results", "数量上限"),
                config.plugins.web_images.max_results.to_string(),
            ),
            Field::boolean(
                t("Safe search", "安全搜索"),
                config.plugins.web_images.safe_search,
            ),
            Field::boolean(
                t("Automatic preview", "自动预览"),
                config.plugins.web_images.auto_preview,
            ),
            Field::new(
                t("Default preview count", "默认预览数量"),
                config.plugins.web_images.preview_count.to_string(),
            ),
            Field::new(
                t("Maximum download (MB)", "最大下载 MB"),
                config.plugins.web_images.max_download_mb.to_string(),
            ),
            Field::new(
                t("Timeout (seconds)", "超时秒数"),
                config.plugins.web_images.timeout_seconds.to_string(),
            ),
        ],
        5 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.print_image.enabled),
            Field::new(
                t("Print width percent", "打印宽度百分比"),
                config.plugins.print_image.width_percent.to_string(),
            ),
            Field::new(
                t("Print height percent", "打印高度百分比"),
                config.plugins.print_image.height_percent.to_string(),
            ),
        ],
        6 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.memes.enabled),
            Field::new(
                t("Send width percent", "发送宽度百分比"),
                config.plugins.memes.width_percent.to_string(),
            ),
            Field::new(
                t("Send height percent", "发送高度百分比"),
                config.plugins.memes.height_percent.to_string(),
            ),
            Field::new(
                t("Maximum image size (MB)", "最大图片 MB"),
                config.plugins.memes.max_image_mb.to_string(),
            ),
            Field::new(
                t("Maximum search results", "搜索最大结果数"),
                config.plugins.memes.search_max_results.to_string(),
            ),
            Field::boolean(
                t("Allow animated GIFs", "允许 GIF 动画"),
                config.plugins.memes.allow_gif_animation,
            ),
            Field::boolean(
                t("Suggest memes automatically", "自动提示发送表情"),
                config.plugins.memes.auto_send_enabled,
            ),
            Field::new(
                t(
                    "Automatic meme suggestion probability",
                    "自动提示发送表情概率",
                ),
                config.plugins.memes.auto_send_probability.to_string(),
            ),
        ],
        7 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.knowledge_base.enabled),
            Field::new(
                t("Knowledge base directory", "知识库目录"),
                config.plugins.knowledge_base.data_dir.clone(),
            ),
            Field::new(
                t("Maximum search results", "搜索最大结果数"),
                config.plugins.knowledge_base.max_search_results.to_string(),
            ),
            Field::new(
                t("Snippet context characters", "片段上下文字数"),
                config
                    .plugins
                    .knowledge_base
                    .snippet_context_chars
                    .to_string(),
            ),
            Field::new(
                t("Proximity window characters", "同窗匹配范围"),
                config
                    .plugins
                    .knowledge_base
                    .proximity_window_chars
                    .to_string(),
            ),
            Field::new(
                t("Maximum lines to read", "读取最大行数"),
                config.plugins.knowledge_base.max_read_lines.to_string(),
            ),
            Field::new(
                t("Maximum file size (KB)", "最大文件 KB"),
                config.plugins.knowledge_base.max_file_size_kb.to_string(),
            ),
            Field::boolean(
                t("Allow AI uploads", "允许 AI 上传"),
                config.plugins.knowledge_base.upload_tool_enabled,
            ),
            Field::boolean(
                t("Enable embedding", "启用 Embedding"),
                config.plugins.knowledge_base.embedding_enabled,
            ),
            Field::new(
                t("Embedding provider/model", "Embedding Provider/模型"),
                kb_embedding_provider_value(config),
            )
            .choices_owned(provider_model_choice_values(config, false))
            .empty_choice_label(t("Embedding not configured", "未配置 Embedding")),
            Field::new(
                t("Semantic chunk size", "语义块大小"),
                config
                    .plugins
                    .knowledge_base
                    .semantic_chunk_chars
                    .to_string(),
            ),
            Field::new(
                t("Semantic chunk overlap", "语义块重叠"),
                config
                    .plugins
                    .knowledge_base
                    .semantic_chunk_overlap
                    .to_string(),
            ),
            Field::new(
                t("Semantic candidates", "语义候选数"),
                config.plugins.knowledge_base.semantic_top_k.to_string(),
            ),
            Field::new(
                t("Minimum semantic score", "语义最低分"),
                config.plugins.knowledge_base.semantic_min_score.to_string(),
            ),
            Field::new(
                t("Strong keyword match threshold", "关键词强命中阈值"),
                config
                    .plugins
                    .knowledge_base
                    .keyword_strong_score_threshold
                    .to_string(),
            ),
            Field::new(
                t("Embedding timeout (seconds)", "Embedding 超时秒数"),
                config
                    .plugins
                    .knowledge_base
                    .embedding_timeout_seconds
                    .to_string(),
            ),
        ],
        8 => vec![Field::boolean(
            t("Enabled", "启用"),
            config.plugins.archlinux.enabled,
        )],
        9 => vec![Field::boolean(
            t("Enabled", "启用"),
            config.plugins.man.enabled,
        )],
        10 => vec![
            Field::boolean(t("Enabled", "启用"), config.plugins.memory.enabled),
            Field::boolean(
                t("Evicted context cache", "上下文弹出缓存"),
                config.plugins.memory.evicted_context_enabled,
            ),
            Field::boolean(
                t("Enable association", "联想启用"),
                config.plugins.memory.association_enabled,
            ),
            Field::boolean(
                t("Automatic diary", "自动日记"),
                config.plugins.memory.auto_diary_enabled,
            ),
            Field::boolean(
                t("Automatic fact memory", "自动知识记忆"),
                config.plugins.memory.auto_fact_enabled,
            ),
            Field::new(
                t("Associated facts", "联想知识条数"),
                config.plugins.memory.association_facts.to_string(),
            ),
            Field::new(
                t("Associated events", "联想事件条数"),
                config.plugins.memory.association_episodes.to_string(),
            ),
            Field::new(
                t("Association character limit", "联想字符上限"),
                config.plugins.memory.association_max_chars.to_string(),
            ),
            Field::boolean(
                t("Enable forgetting", "遗忘启用"),
                config.plugins.memory.forgetting_enabled,
            ),
            Field::new(
                t("Forgetting half-life (days)", "遗忘半衰期天"),
                config.plugins.memory.forgetting_half_life_days.to_string(),
            ),
            Field::new(
                t("Minimum forgetting strength", "遗忘最低强度"),
                config.plugins.memory.forgetting_min_strength.to_string(),
            ),
            Field::new(
                t("Recall boost strength", "回忆增强强度"),
                config.plugins.memory.forgetting_review_boost.to_string(),
            ),
        ],
        11 => vec![Field::boolean(
            t("Enabled", "启用"),
            config.plugins.package_advisor.enabled,
        )],
        12 => vec![
            Field::boolean(
                t("Enabled", "启用"),
                config
                    .plugins
                    .deep_research_linux_game_compatibility
                    .enabled,
            ),
            Field::new(
                t("Maximum subagent tool steps", "子代理最大工具次数"),
                config
                    .plugins
                    .deep_research_linux_game_compatibility
                    .max_tool_steps
                    .to_string(),
            ),
        ],
        _ => vec![Field::boolean(
            t("Enabled", "启用"),
            plugin_enabled(config, index),
        )],
    }
}

fn apply_plugin_fields(config: &mut AppConfig, index: usize, fields: &[Field]) -> Result<()> {
    match index {
        0 => {
            config.plugins.web.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.web.max_results = fields[1].value.trim().parse::<usize>()?.clamp(1, 10);
            config.plugins.web.tavily_api_keys = parse_key_list(&fields[2].value);
            config.plugins.web.firecrawl_api_keys = parse_key_list(&fields[3].value);
            config.plugins.web.anysearch_api_keys = parse_key_list(&fields[4].value);
            config.plugins.web.searxng_base_url =
                fields[5].value.trim().trim_end_matches('/').to_string();
        }
        1 => {
            config.plugins.deep_research.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.deep_research.output_dir = fields[1].value.trim().to_string();
            config.plugins.deep_research.thinking_depth = fields[2].value.trim().to_string();
            config.plugins.deep_research.max_review_revisions = fields[3].value.trim().parse()?;
            config.plugins.deep_research.max_tool_steps_per_round =
                fields[4].value.trim().parse()?;
            config.plugins.deep_research.max_final_answer_chars = fields[5].value.trim().parse()?;
            config.plugins.deep_research.tool_call_timeout_seconds =
                fields[6].value.trim().parse()?;
            config.plugins.deep_research.show_progress = parse_bool_field(&fields[7].value)?;
        }
        2 => {
            config.plugins.vision.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.vision.prefer_current_multimodal_model =
                parse_bool_field(&fields[1].value)?;
            let (provider_id, model) = parse_provider_model_choice(&fields[2].value);
            config.plugins.vision.vision_provider_id = provider_id;
            config.plugins.vision.vision_model = model;
        }
        3 => {
            config.plugins.image_generation.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.image_generation.provider_type = fields[1].value.trim().to_string();
            config.plugins.image_generation.base_url =
                fields[2].value.trim().trim_end_matches('/').to_string();
            config.plugins.image_generation.api_keys = parse_key_list(&fields[3].value);
            config.plugins.image_generation.model = fields[4].value.trim().to_string();
            config.plugins.image_generation.default_aspect_ratio =
                fields[5].value.trim().to_string();
            config.plugins.image_generation.default_resolution = fields[6].value.trim().to_string();
            config.plugins.image_generation.output_dir = fields[7].value.trim().to_string();
            config.plugins.image_generation.auto_print = parse_bool_field(&fields[8].value)?;
            config.plugins.image_generation.timeout_seconds = fields[9].value.trim().parse()?;
        }
        4 => {
            config.plugins.web_images.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.web_images.source_mode = match fields[1].value.trim() {
                "auto" | "global" | "mainland" => fields[1].value.trim().to_string(),
                other => {
                    if is_zh() {
                        anyhow::bail!("未知搜图来源模式: {other}")
                    } else {
                        anyhow::bail!("Unknown image search source mode: {other}")
                    }
                }
            };
            config.plugins.web_images.vision_screening_enabled =
                parse_bool_field(&fields[2].value)?;
            config.plugins.web_images.max_results =
                fields[3].value.trim().parse::<usize>()?.clamp(1, 10);
            config.plugins.web_images.safe_search = parse_bool_field(&fields[4].value)?;
            config.plugins.web_images.auto_preview = parse_bool_field(&fields[5].value)?;
            config.plugins.web_images.preview_count =
                fields[6].value.trim().parse::<usize>()?.min(5);
            config.plugins.web_images.max_download_mb =
                fields[7].value.trim().parse::<f64>()?.clamp(0.1, 50.0);
            config.plugins.web_images.timeout_seconds =
                fields[8].value.trim().parse::<u64>()?.clamp(5, 120);
        }
        5 => {
            config.plugins.print_image.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.print_image.width_percent = fields[1].value.trim().parse::<u8>()?;
            config.plugins.print_image.height_percent = fields[2].value.trim().parse::<u8>()?;
        }
        6 => {
            config.plugins.memes.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.memes.width_percent =
                fields[1].value.trim().parse::<u8>()?.clamp(1, 100);
            config.plugins.memes.height_percent =
                fields[2].value.trim().parse::<u8>()?.clamp(1, 100);
            config.plugins.memes.max_image_mb =
                fields[3].value.trim().parse::<u64>()?.clamp(1, 100);
            config.plugins.memes.search_max_results =
                fields[4].value.trim().parse::<usize>()?.clamp(1, 3);
            config.plugins.memes.allow_gif_animation = parse_bool_field(&fields[5].value)?;
            config.plugins.memes.auto_send_enabled = parse_bool_field(&fields[6].value)?;
            config.plugins.memes.auto_send_probability =
                fields[7].value.trim().parse::<f32>()?.clamp(0.0, 1.0);
        }
        7 => {
            config.plugins.knowledge_base.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.knowledge_base.data_dir = fields[1].value.trim().to_string();
            config.plugins.knowledge_base.max_search_results = fields[2].value.trim().parse()?;
            config.plugins.knowledge_base.snippet_context_chars = fields[3].value.trim().parse()?;
            config.plugins.knowledge_base.proximity_window_chars =
                fields[4].value.trim().parse()?;
            config.plugins.knowledge_base.max_read_lines = fields[5].value.trim().parse()?;
            config.plugins.knowledge_base.max_file_size_kb = fields[6].value.trim().parse()?;
            config.plugins.knowledge_base.upload_tool_enabled = parse_bool_field(&fields[7].value)?;
            config.plugins.knowledge_base.embedding_enabled = parse_bool_field(&fields[8].value)?;
            let (provider_id, model) = parse_provider_model_choice(&fields[9].value);
            config.plugins.knowledge_base.embedding_provider_id = provider_id;
            config.plugins.knowledge_base.embedding_model = model;
            config.plugins.knowledge_base.semantic_chunk_chars = fields[10].value.trim().parse()?;
            config.plugins.knowledge_base.semantic_chunk_overlap =
                fields[11].value.trim().parse()?;
            config.plugins.knowledge_base.semantic_top_k = fields[12].value.trim().parse()?;
            config.plugins.knowledge_base.semantic_min_score = fields[13].value.trim().parse()?;
            config.plugins.knowledge_base.keyword_strong_score_threshold =
                fields[14].value.trim().parse()?;
            config.plugins.knowledge_base.embedding_timeout_seconds =
                fields[15].value.trim().parse()?;
        }
        8 => {
            config.plugins.archlinux.enabled = parse_bool_field(&fields[0].value)?;
        }
        9 => {
            config.plugins.man.enabled = parse_bool_field(&fields[0].value)?;
        }
        10 => {
            config.plugins.memory.enabled = parse_bool_field(&fields[0].value)?;
            config.plugins.memory.evicted_context_enabled = parse_bool_field(&fields[1].value)?;
            config.plugins.memory.association_enabled = parse_bool_field(&fields[2].value)?;
            config.plugins.memory.auto_diary_enabled = parse_bool_field(&fields[3].value)?;
            config.plugins.memory.auto_fact_enabled = parse_bool_field(&fields[4].value)?;
            config.plugins.memory.auto_skill_enabled = false;
            config.plugins.memory.association_facts = fields[5].value.trim().parse::<usize>()?;
            config.plugins.memory.association_episodes = fields[6].value.trim().parse::<usize>()?;
            config.plugins.memory.association_max_chars =
                fields[7].value.trim().parse::<usize>()?;
            config.plugins.memory.forgetting_enabled = parse_bool_field(&fields[8].value)?;
            config.plugins.memory.forgetting_half_life_days =
                fields[9].value.trim().parse::<f64>()?;
            config.plugins.memory.forgetting_min_strength =
                fields[10].value.trim().parse::<f64>()?;
            config.plugins.memory.forgetting_review_boost =
                fields[11].value.trim().parse::<f64>()?;
        }
        11 => {
            config.plugins.package_advisor.enabled = parse_bool_field(&fields[0].value)?;
        }
        12 => {
            config
                .plugins
                .deep_research_linux_game_compatibility
                .enabled = parse_bool_field(&fields[0].value)?;
            config
                .plugins
                .deep_research_linux_game_compatibility
                .max_tool_steps = fields[1].value.trim().parse::<usize>()?.clamp(1, 500);
        }
        _ => {
            let value = parse_bool_field(&fields[0].value)?;
            if plugin_enabled(config, index) != value {
                toggle_plugin(config, index);
            }
        }
    }
    Ok(())
}

fn edit_custom_prompts(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &mut AppConfig,
) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let persona = if config.prompt.active_persona.trim().is_empty() {
            "Miyu".to_string()
        } else {
            persona_display_name(&config.prompt.active_persona).to_string()
        };
        let options = [
            format!(
                "{} ({}: {persona})",
                t("AI persona", "AI 人格"),
                t("Current", "当前")
            ),
            t("User identity", "用户身份").to_string(),
        ];
        draw_menu(
            stdout,
            t(" CUSTOM PROMPTS ", " 自定义提示词 "),
            &options,
            selected,
            t("[Enter]select [q]back", "[Enter]选择 [q]返回"),
        )?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter if selected == 0 => edit_personas(stdout, paths, config)?,
            KeyCode::Enter if selected == 1 => edit_identities(stdout, paths, config)?,
            _ => {}
        }
    }
}

fn edit_personas(stdout: &mut io::Stdout, paths: &MiyuPaths, config: &mut AppConfig) -> Result<()> {
    std::fs::create_dir_all(config.prompts_dir_path(paths))?;
    let mut selected = 0usize;
    loop {
        let personas = list_personas(paths, config)?;
        let mut options = Vec::with_capacity(personas.len() + 1);
        let default_marker = if config.prompt.active_persona.trim().is_empty() {
            "* "
        } else {
            "  "
        };
        options.push(format!("{default_marker}Miyu"));
        options.extend(personas.iter().map(|name| {
            let display = persona_display_name(name);
            if *name == config.prompt.active_persona {
                format!("* {display}")
            } else {
                format!("  {display}")
            }
        }));
        selected = selected.min(options.len().saturating_sub(1));
        draw_menu(
            stdout,
            t(" AI PERSONA ", " AI 人格 "),
            &options,
            selected,
            t(
                "[Tab]activate [Enter]edit [a]add [d]delete [j/k]move [q]back",
                "[Tab]激活 [Enter]编辑 [a]新增 [d]删除 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Tab => {
                config.prompt.active_persona = if selected == 0 {
                    String::new()
                } else {
                    personas.get(selected - 1).cloned().unwrap_or_default()
                };
            }
            KeyCode::Char('a') => {
                if let Some(name) = new_persona(stdout, paths, config)? {
                    config.prompt.active_persona = name;
                }
            }
            KeyCode::Enter if selected > 0 => {
                if let Some(name) = personas.get(selected - 1) {
                    if let Some(new_name) = edit_persona(stdout, paths, config, name)? {
                        move_persona_scope(paths, config, name, &new_name)?;
                        if config.prompt.active_persona == *name {
                            config.prompt.active_persona = new_name;
                        }
                    }
                }
            }
            KeyCode::Char('d') if selected > 0 => {
                if let Some(name) = personas.get(selected - 1) {
                    let path = config.persona_path(paths, name);
                    if path.exists() {
                        std::fs::remove_file(path)?;
                    }
                    remove_persona_scope(paths, config, name)?;
                    if config.prompt.active_persona == *name {
                        config.prompt.active_persona.clear();
                    }
                    selected = selected.saturating_sub(1);
                }
            }
            _ => {}
        }
    }
}

fn new_persona(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &AppConfig,
) -> Result<Option<String>> {
    edit_prompt_file_form(
        stdout,
        t(" NEW PERSONA ", " 新建人格 "),
        None,
        String::new(),
        |name, content| write_persona(paths, config, name, content),
    )
}

fn edit_persona(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &AppConfig,
    current_name: &str,
) -> Result<Option<String>> {
    let content = read_persona(paths, config, current_name)?;
    edit_prompt_file_form(
        stdout,
        t(" EDIT PERSONA ", " 编辑人格 "),
        Some(current_name),
        content,
        |name, content| {
            if name != current_name {
                let old_path = config.persona_path(paths, current_name);
                if old_path.exists() {
                    std::fs::remove_file(old_path)?;
                }
            }
            write_persona(paths, config, name, content)
        },
    )
}

fn move_persona_scope(
    paths: &MiyuPaths,
    config: &AppConfig,
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    if old_name == new_name {
        return Ok(());
    }
    move_dir_if_exists(
        config.persona_memory_data_dir(paths, old_name),
        config.persona_memory_data_dir(paths, new_name),
    )?;
    move_dir_if_exists(
        config.persona_memory_state_dir(paths, old_name),
        config.persona_memory_state_dir(paths, new_name),
    )?;
    move_dir_if_exists(
        config.persona_skills_dir(paths, old_name),
        config.persona_skills_dir(paths, new_name),
    )?;
    Ok(())
}

fn remove_persona_scope(paths: &MiyuPaths, config: &AppConfig, name: &str) -> Result<()> {
    remove_dir_if_exists(config.persona_memory_data_dir(paths, name))?;
    remove_dir_if_exists(config.persona_memory_state_dir(paths, name))?;
    remove_dir_if_exists(config.persona_skills_dir(paths, name))?;
    Ok(())
}

fn move_dir_if_exists(from: PathBuf, to: PathBuf) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if to.exists() {
        std::fs::remove_dir_all(&to)?;
    }
    std::fs::rename(from, to)?;
    Ok(())
}

fn remove_dir_if_exists(path: PathBuf) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn edit_identities(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &mut AppConfig,
) -> Result<()> {
    std::fs::create_dir_all(config.identities_dir_path(paths))?;
    let mut selected = 0usize;
    loop {
        let identities = list_identities(paths, config)?;
        let mut options = Vec::with_capacity(identities.len() + 1);
        let default_marker = if config.prompt.active_identity.trim().is_empty() {
            "* "
        } else {
            "  "
        };
        options.push(format!(
            "{default_marker}{}",
            t("Do not use a user identity", "不使用用户身份")
        ));
        options.extend(identities.iter().map(|name| {
            let display = persona_display_name(name);
            if *name == config.prompt.active_identity {
                format!("* {display}")
            } else {
                format!("  {display}")
            }
        }));
        selected = selected.min(options.len().saturating_sub(1));
        draw_menu(
            stdout,
            t(" USER IDENTITY ", " 用户身份 "),
            &options,
            selected,
            t(
                "[Tab]activate [Enter]edit [a]add [d]delete [j/k]move [q]back",
                "[Tab]激活 [Enter]编辑 [a]新增 [d]删除 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Tab => {
                config.prompt.active_identity = if selected == 0 {
                    String::new()
                } else {
                    identities.get(selected - 1).cloned().unwrap_or_default()
                };
            }
            KeyCode::Char('a') => {
                if let Some(name) = new_identity(stdout, paths, config)? {
                    config.prompt.active_identity = name;
                }
            }
            KeyCode::Enter if selected > 0 => {
                if let Some(name) = identities.get(selected - 1) {
                    if let Some(new_name) = edit_identity(stdout, paths, config, name)? {
                        if config.prompt.active_identity == *name {
                            config.prompt.active_identity = new_name;
                        }
                    }
                }
            }
            KeyCode::Char('d') if selected > 0 => {
                if let Some(name) = identities.get(selected - 1) {
                    let path = config.identity_path(paths, name);
                    if path.exists() {
                        std::fs::remove_file(path)?;
                    }
                    if config.prompt.active_identity == *name {
                        config.prompt.active_identity.clear();
                    }
                    selected = selected.saturating_sub(1);
                }
            }
            _ => {}
        }
    }
}

fn new_identity(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &AppConfig,
) -> Result<Option<String>> {
    edit_prompt_file_form(
        stdout,
        t(" NEW IDENTITY ", " 新建用户身份 "),
        None,
        String::new(),
        |name, content| write_identity(paths, config, name, content),
    )
}

fn edit_identity(
    stdout: &mut io::Stdout,
    paths: &MiyuPaths,
    config: &AppConfig,
    current_name: &str,
) -> Result<Option<String>> {
    let content = read_identity(paths, config, current_name)?;
    edit_prompt_file_form(
        stdout,
        t(" EDIT IDENTITY ", " 编辑用户身份 "),
        Some(current_name),
        content,
        |name, content| {
            if name != current_name {
                let old_path = config.identity_path(paths, current_name);
                if old_path.exists() {
                    std::fs::remove_file(old_path)?;
                }
            }
            write_identity(paths, config, name, content)
        },
    )
}

fn list_identities(paths: &MiyuPaths, config: &AppConfig) -> Result<Vec<String>> {
    list_markdown_files(&config.identities_dir_path(paths))
}

fn read_identity(paths: &MiyuPaths, config: &AppConfig, name: &str) -> Result<String> {
    let path = config.identity_path(paths, name);
    if path.exists() {
        Ok(std::fs::read_to_string(path)?)
    } else {
        Ok(String::new())
    }
}

fn write_identity(paths: &MiyuPaths, config: &AppConfig, name: &str, content: &str) -> Result<()> {
    let path = config.identity_path(paths, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format_text_file(content))?;
    Ok(())
}

fn edit_prompt_file_form<F>(
    stdout: &mut io::Stdout,
    title: &str,
    current_name: Option<&str>,
    content: String,
    write: F,
) -> Result<Option<String>>
where
    F: FnOnce(&str, &str) -> Result<()>,
{
    let mut fields = vec![
        Field::new(
            t("Name", "名称"),
            current_name
                .map(persona_display_name)
                .unwrap_or("")
                .to_string(),
        ),
        Field::textarea(t("Content", "内容"), content),
    ];
    if !run_form(stdout, title, &mut fields)? {
        return Ok(None);
    }
    let name = sanitize_persona_name(&fields[0].value)?;
    write(&name, &fields[1].value)?;
    Ok(Some(name))
}

fn list_personas(paths: &MiyuPaths, config: &AppConfig) -> Result<Vec<String>> {
    list_markdown_files(&config.prompts_dir_path(paths))
}

fn list_markdown_files(dir: &std::path::Path) -> Result<Vec<String>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                names.push(name);
            }
        }
    }
    names.sort();
    Ok(names)
}

fn read_persona(paths: &MiyuPaths, config: &AppConfig, name: &str) -> Result<String> {
    let path = config.persona_path(paths, name);
    if path.exists() {
        Ok(std::fs::read_to_string(path)?)
    } else {
        Ok(String::new())
    }
}

fn write_persona(paths: &MiyuPaths, config: &AppConfig, name: &str, content: &str) -> Result<()> {
    let path = config.persona_path(paths, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format_text_file(content))?;
    Ok(())
}

fn sanitize_persona_name(value: &str) -> Result<String> {
    let mut name = value
        .trim()
        .trim_end_matches(".md")
        .replace(['/', '\\'], "-");
    if name.is_empty() {
        bail!("{}", t("Persona name cannot be empty", "人格名称不能为空"));
    }
    name.push_str(".md");
    Ok(name)
}

fn persona_display_name(name: &str) -> &str {
    name.strip_suffix(".md").unwrap_or(name)
}

fn format_text_file(content: &str) -> String {
    let content = content.trim_end();
    if content.is_empty() {
        String::new()
    } else {
        format!("{content}\n")
    }
}

fn parse_key_list(value: &str) -> Vec<String> {
    value
        .split(|ch| ch == ',' || ch == '\n' || ch == '\r')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

struct ProviderBrowser<'a> {
    paths: &'a MiyuPaths,
    config: &'a mut AppConfig,
    active_col: usize,
    provider_idx: usize,
    provider_scroll: usize,
    org_idx: usize,
    org_scroll: usize,
    model_idx: usize,
    model_scroll: usize,
    filter: String,
    filter_mode: bool,
    raw_models: Vec<String>,
    orgs: Vec<String>,
    models: Vec<ModelEntry>,
    status: String,
    loading: bool,
    fetch_seq: u64,
    fetch_rx: Option<Receiver<FetchResult>>,
}

impl<'a> ProviderBrowser<'a> {
    fn new(paths: &'a MiyuPaths, config: &'a mut AppConfig) -> Self {
        Self {
            paths,
            config,
            active_col: 0,
            provider_idx: 0,
            provider_scroll: 0,
            org_idx: 0,
            org_scroll: 0,
            model_idx: 0,
            model_scroll: 0,
            filter: String::new(),
            filter_mode: false,
            raw_models: Vec::new(),
            orgs: Vec::new(),
            models: Vec::new(),
            status: String::new(),
            loading: false,
            fetch_seq: 0,
            fetch_rx: None,
        }
    }

    fn run(mut self, stdout: &mut io::Stdout) -> Result<()> {
        self.refresh_models();
        loop {
            self.poll_fetch_result();
            self.draw(stdout)?;
            match read_key_with_timeout(if self.loading {
                Some(Duration::from_millis(100))
            } else {
                None
            })? {
                None => continue,
                Some(key) => match key {
                    key if self.filter_mode => self.handle_filter_key(key),
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Left | KeyCode::Char('h') => self.move_left(),
                    KeyCode::Right | KeyCode::Char('l') => self.move_right(),
                    KeyCode::Up | KeyCode::Char('k') => self.move_up(),
                    KeyCode::Down | KeyCode::Char('j') => self.move_down(),
                    KeyCode::Char('/') => {
                        self.filter_mode = true;
                        self.filter.clear();
                        self.rebuild_models();
                    }
                    KeyCode::Char('r') => self.refresh_models(),
                    KeyCode::Char('a') => self.add_provider(stdout)?,
                    KeyCode::Char('d') => self.delete_provider(),
                    KeyCode::Tab if self.active_col == 2 => self.toggle_model_activation(),
                    KeyCode::Enter | KeyCode::Char('i') => self.select_or_edit(stdout)?,
                    _ => {}
                },
            }
        }
    }

    fn handle_filter_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.filter_mode = false;
                self.filter.clear();
            }
            KeyCode::Enter => self.filter_mode = false,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(ch) => self.filter.push(ch),
            _ => {}
        }
        self.rebuild_models();
    }

    fn move_left(&mut self) {
        self.active_col = self.active_col.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.active_col = (self.active_col + 1).min(2);
    }

    fn move_up(&mut self) {
        match self.active_col {
            0 => {
                self.provider_idx = self.provider_idx.saturating_sub(1);
                self.provider_scroll = column_scroll(
                    self.provider_idx,
                    self.provider_scroll,
                    column_visible_rows(),
                );
                self.refresh_models();
            }
            1 => {
                self.org_idx = self.org_idx.saturating_sub(1);
                self.org_scroll =
                    column_scroll(self.org_idx, self.org_scroll, column_visible_rows());
                self.rebuild_models();
            }
            2 => {
                self.model_idx = self.model_idx.saturating_sub(1);
                self.model_scroll =
                    column_scroll(self.model_idx, self.model_scroll, column_visible_rows());
            }
            _ => {}
        }
    }

    fn move_down(&mut self) {
        match self.active_col {
            0 => {
                self.provider_idx =
                    (self.provider_idx + 1).min(self.config.providers.len().saturating_sub(1));
                self.provider_scroll = column_scroll(
                    self.provider_idx,
                    self.provider_scroll,
                    column_visible_rows(),
                );
                self.refresh_models();
            }
            1 => {
                self.org_idx = (self.org_idx + 1).min(self.orgs.len().saturating_sub(1));
                self.org_scroll =
                    column_scroll(self.org_idx, self.org_scroll, column_visible_rows());
                self.rebuild_models();
            }
            2 => {
                self.model_idx = (self.model_idx + 1).min(self.models.len().saturating_sub(1));
                self.model_scroll =
                    column_scroll(self.model_idx, self.model_scroll, column_visible_rows());
            }
            _ => {}
        }
    }

    fn refresh_models(&mut self) {
        self.provider_idx = self
            .provider_idx
            .min(self.config.providers.len().saturating_sub(1));
        self.raw_models.clear();
        self.orgs = vec!["All".to_string()];
        self.models.clear();
        self.fetch_seq += 1;
        if let Some(provider) = self.config.providers.get(self.provider_idx).cloned() {
            let seq = self.fetch_seq;
            let (tx, rx) = mpsc::channel();
            self.fetch_rx = Some(rx);
            self.loading = true;
            self.status = t("Fetching model list...", "正在获取模型列表...").to_string();
            std::thread::spawn(move || {
                let result = fetch_models(&provider).map_err(|err| err.to_string());
                let _ = tx.send((seq, result));
            });
        } else {
            self.fetch_rx = None;
            self.loading = false;
            self.status.clear();
        }
        self.org_idx = 0;
        self.model_idx = 0;
        self.org_scroll = 0;
        self.model_scroll = 0;
    }

    fn poll_fetch_result(&mut self) {
        let Some(rx) = &self.fetch_rx else {
            return;
        };
        let Ok((seq, result)) = rx.try_recv() else {
            return;
        };
        if seq != self.fetch_seq {
            return;
        }
        self.loading = false;
        self.fetch_rx = None;
        match result {
            Ok(models) => {
                self.status = if is_zh() {
                    format!("已获取 {} 个模型", models.len())
                } else {
                    format!("Fetched {} models", models.len())
                };
                self.raw_models = models;
            }
            Err(err) => {
                let status = if is_zh() {
                    format!("获取模型失败: {err}")
                } else {
                    format!("Failed to fetch models: {err}")
                };
                self.status = format_status_line(&status);
                self.raw_models.clear();
            }
        }
        self.rebuild_models();
    }

    fn rebuild_models(&mut self) {
        let filter = self.filter.to_ascii_lowercase();
        let mut grouped: BTreeMap<String, Vec<ModelEntry>> = BTreeMap::new();
        for model in &self.raw_models {
            if !filter.is_empty() && !model.to_ascii_lowercase().contains(&filter) {
                continue;
            }
            let org = model
                .split_once('/')
                .map(|(org, _)| org)
                .unwrap_or("All")
                .to_string();
            let name = model
                .split_once('/')
                .map(|(_, name)| name)
                .unwrap_or(model)
                .to_string();
            grouped
                .entry("All".to_string())
                .or_default()
                .push(ModelEntry::new(model, model));
            if org != "All" {
                grouped
                    .entry(org)
                    .or_default()
                    .push(ModelEntry::new(&name, model));
            }
        }
        self.orgs = grouped.keys().cloned().collect();
        if self.orgs.is_empty() {
            self.orgs.push("All".to_string());
        }
        self.org_idx = self.org_idx.min(self.orgs.len().saturating_sub(1));
        self.models = grouped.remove(&self.orgs[self.org_idx]).unwrap_or_default();
        self.model_idx = self.model_idx.min(self.models.len().saturating_sub(1));
        self.org_scroll = column_scroll(self.org_idx, self.org_scroll, column_visible_rows());
        self.model_scroll = column_scroll(self.model_idx, self.model_scroll, column_visible_rows());
    }

    fn add_provider(&mut self, stdout: &mut io::Stdout) -> Result<()> {
        if let Some(provider) = edit_provider_form(stdout, ProviderConfig::new_custom())? {
            self.config.upsert_provider(provider);
            self.provider_idx = self.config.providers.len().saturating_sub(1);
            self.refresh_models();
        }
        Ok(())
    }

    fn delete_provider(&mut self) {
        if self.config.providers.is_empty() {
            return;
        }
        let removed = self.config.providers.remove(self.provider_idx);
        self.config.remove_provider_references(&removed.id);
        self.provider_idx = self
            .provider_idx
            .min(self.config.providers.len().saturating_sub(1));
        self.refresh_models();
    }

    fn select_or_edit(&mut self, stdout: &mut io::Stdout) -> Result<()> {
        match self.active_col {
            0 => {
                if let Some(provider) = self.config.providers.get(self.provider_idx).cloned() {
                    if let Some(provider) = edit_provider_form(stdout, provider)? {
                        let old_id = self.config.providers[self.provider_idx].id.clone();
                        self.config.providers[self.provider_idx] = provider.clone();
                        if self.config.active_provider == old_id {
                            self.config.active_provider = provider.id.clone();
                        }
                        if old_id != provider.id {
                            self.config
                                .rename_provider_references(&old_id, &provider.id);
                        }
                        self.refresh_models();
                    }
                }
            }
            2 => {
                let mut model_updated = false;
                if let Some(model) = self.models.get(self.model_idx).cloned() {
                    if let Some(provider) = self.config.providers.get_mut(self.provider_idx) {
                        auto_configure_model_tags(self.paths, provider, &model.full);
                    }
                    if let Some(provider) = self.config.providers.get_mut(self.provider_idx) {
                        if edit_model_form(stdout, provider, &model.full)? {
                            self.config.active_provider = provider.id.clone();
                            model_updated = true;
                            self.status = if is_zh() {
                                format!("已更新模型设置: {}", model.full)
                            } else {
                                format!("Updated model settings: {}", model.full)
                            };
                        }
                    }
                }
                if model_updated {
                    self.config.prune_model_references();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn toggle_model_activation(&mut self) {
        if self.active_col != 2 {
            return;
        }
        let mut removed = None;
        if let (Some(provider), Some(model)) = (
            self.config.providers.get_mut(self.provider_idx),
            self.models.get(self.model_idx),
        ) {
            if let Some(index) = provider.models.iter().position(|item| item == &model.full) {
                let provider_id = provider.id.clone();
                let model = model.full.clone();
                provider.models.remove(index);
                if provider.default_model == model {
                    provider.default_model = provider.models.first().cloned().unwrap_or_default();
                }
                self.status = if is_zh() {
                    format!("已取消激活模型: {model}")
                } else {
                    format!("Deactivated model: {model}")
                };
                removed = Some((provider_id, model));
            } else {
                provider.models.push(model.full.clone());
                auto_configure_model_tags(self.paths, provider, &model.full);
                if provider.default_model.trim().is_empty() {
                    provider.default_model = model.full.clone();
                }
                self.status = if is_zh() {
                    format!("已激活模型: {}", model.full)
                } else {
                    format!("Activated model: {}", model.full)
                };
            }
        }
        if let Some((provider_id, model)) = removed {
            self.config
                .remove_active_model_references(&provider_id, &model);
        }
    }

    fn draw(&self, stdout: &mut io::Stdout) -> Result<()> {
        let (cols, rows) = terminal::size()?;
        let inner_x = 0;
        let inner_y = 0;
        let inner_w = cols;
        let inner_h = rows.saturating_sub(2);
        let left_w = inner_w.saturating_mul(28).saturating_div(100).max(20);
        let mid_w = inner_w.saturating_mul(22).saturating_div(100).max(16);
        let right_w = inner_w
            .saturating_sub(left_w)
            .saturating_sub(mid_w)
            .saturating_sub(2)
            .max(18);
        let providers = self
            .config
            .providers
            .iter()
            .map(|provider| {
                let active = if provider.id == self.config.active_provider {
                    "* "
                } else {
                    "  "
                };
                format!("{active}{}", provider.display_name)
            })
            .collect::<Vec<_>>();
        let models = self
            .models
            .iter()
            .map(|model| {
                let active = self
                    .config
                    .providers
                    .get(self.provider_idx)
                    .map(|provider| provider.models.iter().any(|item| item == &model.full))
                    .unwrap_or(false);
                format!("{} {}", if active { "[*]" } else { "[ ]" }, model.name)
            })
            .collect::<Vec<_>>();
        let orgs = self
            .orgs
            .iter()
            .map(|org| {
                if org == "All" {
                    t("All", "全部").to_string()
                } else {
                    org.clone()
                }
            })
            .collect::<Vec<_>>();

        queue!(stdout, Clear(ClearType::All))?;
        draw_column(
            stdout,
            inner_x,
            inner_y,
            left_w,
            inner_h,
            t(" PROVIDERS ", " 供应商 "),
            &providers,
            self.provider_idx,
            self.provider_scroll,
            self.active_col == 0,
        )?;
        draw_column(
            stdout,
            inner_x + left_w + 1,
            inner_y,
            mid_w,
            inner_h,
            t(" ORGANIZATION ", " 组织 "),
            &orgs,
            self.org_idx,
            self.org_scroll,
            self.active_col == 1,
        )?;
        let title = if self.filter.is_empty() {
            t(" MODELS ", " 模型 ").to_string()
        } else if is_zh() {
            format!(" 模型 /{} ", self.filter)
        } else {
            format!(" MODELS /{} ", self.filter)
        };
        draw_column(
            stdout,
            inner_x + left_w + mid_w + 2,
            inner_y,
            right_w,
            inner_h,
            &title,
            &models,
            self.model_idx,
            self.model_scroll,
            self.active_col == 2,
        )?;
        let help = if self.filter_mode {
            if is_zh() {
                format!("搜索: {}_  [Enter]确认 [Esc]取消", self.filter)
            } else {
                format!("Search: {}_  [Enter]confirm [Esc]cancel", self.filter)
            }
        } else {
            t(
                "[h/l]column [j/k]move [Tab]activate model [Enter]model settings [/]search [r]refresh [a]add [d]delete [q]back",
                "[h/l]切栏 [j/k]移动 [Tab]激活模型 [Enter]模型设置 [/]搜索 [r]刷新 [a]添加 [d]删除 [q]返回",
            )
            .to_string()
        };
        let status = if self.loading {
            format!("{}", self.status)
        } else {
            self.status.clone()
        };
        queue!(
            stdout,
            MoveTo(0, rows.saturating_sub(2)),
            Clear(ClearType::CurrentLine),
            Print(truncate(&status, cols as usize))
        )?;
        queue!(
            stdout,
            MoveTo(0, rows.saturating_sub(1)),
            Clear(ClearType::CurrentLine),
            Print(truncate(&help, cols as usize))
        )?;
        stdout.flush()?;
        Ok(())
    }
}

type FetchResult = (u64, Result<Vec<String>, String>);

fn format_status_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone)]
struct ModelEntry {
    name: String,
    full: String,
}

impl ModelEntry {
    fn new(name: &str, full: &str) -> Self {
        Self {
            name: name.to_string(),
            full: full.to_string(),
        }
    }
}

fn fetch_models(provider: &ProviderConfig) -> Result<Vec<String>> {
    let api_key = provider.api_key.as_deref().unwrap_or_default();
    let mut api_key = if let Some(env_name) = api_key.strip_prefix("$env:") {
        std::env::var(env_name).unwrap_or_default()
    } else {
        api_key.to_string()
    };
    if api_key.is_empty() && provider.is_opencode_zen() {
        api_key = "public".to_string();
    }
    let url = models_url(&provider.base_url);
    let mut request = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(provider.timeout_seconds))
        .build()?
        .get(url)
        .header("Accept", "application/json")
        .header("User-Agent", "miyu-config");
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send()?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("{status}: {body}");
    }
    let parsed: ModelsResponse = serde_json::from_str(&body)?;
    Ok(parsed
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|id| !id.is_empty())
        .collect())
}

fn auto_configure_model_tags(paths: &MiyuPaths, provider: &mut ProviderConfig, model: &str) {
    if provider.model_modalities.contains_key(model) {
        return;
    }
    if let Some(modalities) =
        crate::models_cache::input_modalities_blocking(paths, &provider.id, model)
            .filter(|modalities| !modalities.is_empty())
    {
        provider
            .model_modalities
            .insert(model.to_string(), modalities);
    }
}

fn models_url(base_url: &str) -> String {
    let mut url = base_url.trim().trim_end_matches('/').to_string();
    if url.ends_with("/chat/completions") {
        url.truncate(url.len() - "/chat/completions".len());
    }
    if url.ends_with("/v1") {
        format!("{url}/models")
    } else {
        format!("{url}/v1/models")
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Deserialize)]
struct ModelInfo {
    id: String,
}

fn select_active_provider(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let mut choices = config.text_provider_model_choices();
    if choices.is_empty() {
        message(
            stdout,
            t(
                "No text models are selected. Activate one with Tab under Providers and models first.",
                "没有已勾选的文本模型，请先在供应商和模型里用 Tab 激活模型。",
            ),
        )?;
        return Ok(());
    }
    let mut selected = choices
        .iter()
        .position(|choice| config.is_active_provider_model(&choice.provider_id, &choice.model))
        .unwrap_or(0);
    loop {
        let options = choices
            .iter()
            .map(|choice| {
                let marker = if config.is_active_provider_model(&choice.provider_id, &choice.model)
                {
                    "[*] "
                } else {
                    "[ ] "
                };
                format!("{marker}{}", choice.label())
            })
            .collect::<Vec<_>>();
        draw_menu(
            stdout,
            t(" SELECT TEXT MODEL ", " 选择文本模型 "),
            &options,
            selected,
            t(
                "[Tab]activate/deactivate [Enter/q]confirm [d]remove",
                "[Tab]激活/取消 [Enter/q]确认 [d]移除",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => return Ok(()),
            KeyCode::Tab => {
                let choice = choices[selected].clone();
                config.toggle_active_provider_model(&choice.provider_id, &choice.model)?;
            }
            KeyCode::Char('d') => {
                let choice = choices[selected].clone();
                config.remove_active_provider_model(&choice.provider_id, &choice.model)?;
                choices = config.text_provider_model_choices();
                if choices.is_empty() {
                    message(
                        stdout,
                        t(
                            "The active model was removed; no models are currently available.",
                            "已移除激活模型，当前没有可用模型。",
                        ),
                    )?;
                    return Ok(());
                }
                selected = selected.min(choices.len().saturating_sub(1));
            }
            _ => {}
        }
    }
}

fn subagent_tiers_label(config: &AppConfig) -> String {
    let counts = crate::config::ModelTier::ALL.map(|tier| config.subagent_tier_choices(tier).len());
    if counts.iter().all(|count| *count == 0) {
        t("not configured", "未配置").to_string()
    } else {
        format!(
            "cheap:{} balanced:{} strong:{}",
            counts[0], counts[1], counts[2]
        )
    }
}

fn tier_display_name(tier: crate::config::ModelTier) -> &'static str {
    use crate::config::ModelTier;
    match tier {
        ModelTier::Cheap => "cheap",
        ModelTier::Balanced => "balanced",
        ModelTier::Strong => "strong",
    }
}

/// Tier pool overview: pick a tier, then toggle models for it. Subagents
/// choose a tier by task complexity; unconfigured pools fall back to the
/// main model pool.
fn select_subagent_tiers(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    use crate::config::ModelTier;
    let mut selected = 0usize;
    loop {
        let options = ModelTier::ALL
            .iter()
            .map(|tier| {
                let pool = config.subagent_tier_choices(*tier);
                let summary = if pool.is_empty() {
                    t("fallback to main model", "回退主模型").to_string()
                } else {
                    pool.iter()
                        .map(|choice| choice.model.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let hint = match tier {
                    ModelTier::Cheap => t("simple tasks", "简单任务"),
                    ModelTier::Balanced => t("normal tasks", "普通任务"),
                    ModelTier::Strong => t("complex tasks", "复杂任务"),
                };
                format!("{} ({hint}): {summary}", tier_display_name(*tier))
            })
            .collect::<Vec<_>>();
        draw_menu(
            stdout,
            t(" SUBAGENT TIER POOLS ", " 子代理档位池 "),
            &options,
            selected,
            t(
                "[Enter]configure tier [j/k]move [q]back",
                "[Enter]配置该档位 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => {
                select_subagent_tier_models(stdout, config, ModelTier::ALL[selected])?
            }
            _ => {}
        }
    }
}

/// Model multi-select for one tier pool, mirroring the text-model picker:
/// candidates are the configured text models, Tab toggles membership.
fn select_subagent_tier_models(
    stdout: &mut io::Stdout,
    config: &mut AppConfig,
    tier: crate::config::ModelTier,
) -> Result<()> {
    let choices = config.text_provider_model_choices();
    if choices.is_empty() {
        message(
            stdout,
            t(
                "No text models are configured. Add models under Providers and models first.",
                "没有可用的文本模型，请先在供应商和模型里添加模型。",
            ),
        )?;
        return Ok(());
    }
    let mut selected = 0usize;
    let title = format!(
        " {} · {} ",
        t("TIER POOL", "档位池"),
        tier_display_name(tier)
    );
    loop {
        let options = choices
            .iter()
            .map(|choice| {
                let marker =
                    if config.is_subagent_tier_model(tier, &choice.provider_id, &choice.model) {
                        "[*] "
                    } else {
                        "[ ] "
                    };
                format!("{marker}{}", choice.label())
            })
            .collect::<Vec<_>>();
        draw_menu(
            stdout,
            &title,
            &options,
            selected,
            t(
                "[Tab]add/remove [Enter/q]confirm",
                "[Tab]加入/移出 [Enter/q]确认",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Tab => {
                let choice = choices[selected].clone();
                config.toggle_subagent_tier_model(tier, &choice.provider_id, &choice.model)?;
            }
            _ => {}
        }
    }
}

fn platforms_label(config: &AppConfig) -> String {
    if config.platforms.qq.enabled {
        t("Tencent QQ enabled", "腾讯 QQ 已启用").to_string()
    } else {
        t("disabled", "未启用").to_string()
    }
}

fn select_platforms(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let state = if config.platforms.qq.enabled {
            t("enabled", "已启用")
        } else {
            t("disabled", "未启用")
        };
        let options = [format!("{}: {state}", t("Tencent QQ", "腾讯 QQ"))];
        draw_menu(
            stdout,
            t(" IM PLATFORMS ", " 接入通讯平台 "),
            &options,
            selected,
            t(
                "[Enter]configure [j/k]move [q]back",
                "[Enter]配置 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => edit_qq(stdout, config)?,
            _ => {}
        }
    }
}

fn enabled_label(value: bool) -> &'static str {
    if value {
        t("enabled", "已启用")
    } else {
        t("disabled", "已禁用")
    }
}

fn edit_qq(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let qq = &config.platforms.qq;
        let options = vec![
            format!(
                "{}: {}",
                t("Enabled", "是否启用"),
                enabled_label(qq.enabled)
            ),
            format!(
                "{}: {}",
                t("Reverse WebSocket port", "反向 WebSocket 端口"),
                qq.reverse_ws_port
            ),
            format!(
                "{}: {}",
                t("Reverse WebSocket token", "反向 WebSocket 验证 Token"),
                if qq.access_token.is_empty() {
                    t("empty", "未设置")
                } else {
                    "********"
                }
            ),
            format!(
                "{}: {}",
                t("Administrator QQ ids", "管理员 QQ 号"),
                qq.admin_users.len()
            ),
            format!(
                "{}: {}",
                t(
                    "Allow non-admin computer access",
                    "是否允许非管理员使用电脑"
                ),
                enabled_label(qq.allow_non_admin_host_tools)
            ),
            format!(
                "{}: {}",
                t("Private whitelist", "私聊白名单"),
                qq.private_chats.whitelist.len()
            ),
            format!(
                "{}: {}",
                t("Allow non-whitelist private chats", "是否允许非白名单私聊"),
                enabled_label(qq.private_chats.allow_non_whitelist)
            ),
            format!(
                "{}: {}",
                t(
                    "Non-whitelist private messages/min",
                    "非白名单私聊限流（每分钟）"
                ),
                qq.private_chats.non_whitelist_rate_per_minute
            ),
            format!(
                "{}: {}",
                t("Group whitelist", "群聊白名单"),
                qq.group_chats.whitelist.len()
            ),
            format!(
                "{}: {}",
                t("Additional group wake keywords", "额外群聊触发关键词"),
                qq.group_chats.trigger_keywords.len()
            ),
            format!(
                "{}: {}",
                t("Whitelist-group messages/min", "白名单群聊限流（每分钟）"),
                qq.group_chats.whitelist_rate_per_minute
            ),
            format!(
                "{}: {}",
                t("Allow non-whitelist groups", "是否允许非白名单群聊"),
                enabled_label(qq.group_chats.allow_non_whitelist)
            ),
            format!(
                "{}: {}",
                t(
                    "Non-whitelist-group messages/min",
                    "非白名单群聊限流（每分钟）"
                ),
                qq.group_chats.non_whitelist_rate_per_minute
            ),
            format!(
                "{}: {}",
                t("Private/group conversation settings", "私聊/群聊专属配置"),
                qq.conversations.len()
            ),
            t("QQ plugins", "QQ 插件配置").to_string(),
            t("Advanced settings", "高级设置").to_string(),
        ];
        draw_menu(
            stdout,
            t(" TENCENT QQ ", " 腾讯 QQ "),
            &options,
            selected,
            t(
                "@-mentions always wake group chats; computer access applies only to private-whitelist users",
                "群聊始终支持 @ 触发；非管理员电脑权限仅对私聊白名单用户生效",
            ),
        )?;
        let key = read_key()?;
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter | KeyCode::Char(' ') => match selected {
                0 => config.platforms.qq.enabled = !config.platforms.qq.enabled,
                1 if matches!(key, KeyCode::Enter) => {
                    if let Some(value) = edit_u16_value(
                        stdout,
                        t("Reverse WebSocket port", "反向 WebSocket 端口"),
                        config.platforms.qq.reverse_ws_port,
                    )? {
                        if value == 0 {
                            message(
                                stdout,
                                t(
                                    "Port must be between 1 and 65535.",
                                    "端口必须在 1 到 65535 之间。",
                                ),
                            )?;
                        } else {
                            config.platforms.qq.reverse_ws_port = value;
                        }
                    }
                }
                2 if matches!(key, KeyCode::Enter) => edit_qq_token(stdout, config)?,
                3 if matches!(key, KeyCode::Enter) => edit_qq_id_list(
                    stdout,
                    t(" ADMINISTRATORS ", " 管理员 QQ 号 "),
                    t("QQ id", "QQ 号"),
                    &mut config.platforms.qq.admin_users,
                )?,
                4 => {
                    config.platforms.qq.allow_non_admin_host_tools =
                        !config.platforms.qq.allow_non_admin_host_tools
                }
                5 if matches!(key, KeyCode::Enter) => edit_qq_id_list(
                    stdout,
                    t(" PRIVATE WHITELIST ", " 私聊白名单 "),
                    t("QQ id", "QQ 号"),
                    &mut config.platforms.qq.private_chats.whitelist,
                )?,
                6 => {
                    config.platforms.qq.private_chats.allow_non_whitelist =
                        !config.platforms.qq.private_chats.allow_non_whitelist
                }
                7 if matches!(key, KeyCode::Enter) => {
                    if let Some(value) = edit_u32_value(
                        stdout,
                        t(
                            "Messages per minute (0 = unlimited)",
                            "每分钟消息上限（0 = 不限）",
                        ),
                        config
                            .platforms
                            .qq
                            .private_chats
                            .non_whitelist_rate_per_minute,
                    )? {
                        config
                            .platforms
                            .qq
                            .private_chats
                            .non_whitelist_rate_per_minute = value;
                    }
                }
                8 if matches!(key, KeyCode::Enter) => edit_qq_id_list(
                    stdout,
                    t(" GROUP WHITELIST ", " 群聊白名单 "),
                    t("Group id", "群号"),
                    &mut config.platforms.qq.group_chats.whitelist,
                )?,
                9 if matches!(key, KeyCode::Enter) => edit_keyword_list(
                    stdout,
                    &mut config.platforms.qq.group_chats.trigger_keywords,
                )?,
                10 if matches!(key, KeyCode::Enter) => {
                    if let Some(value) = edit_u32_value(
                        stdout,
                        t(
                            "Messages per minute (0 = unlimited)",
                            "每分钟消息上限（0 = 不限）",
                        ),
                        config.platforms.qq.group_chats.whitelist_rate_per_minute,
                    )? {
                        config.platforms.qq.group_chats.whitelist_rate_per_minute = value;
                    }
                }
                11 => {
                    config.platforms.qq.group_chats.allow_non_whitelist =
                        !config.platforms.qq.group_chats.allow_non_whitelist
                }
                12 if matches!(key, KeyCode::Enter) => {
                    if let Some(value) = edit_u32_value(
                        stdout,
                        t(
                            "Messages per minute (0 = unlimited)",
                            "每分钟消息上限（0 = 不限）",
                        ),
                        config
                            .platforms
                            .qq
                            .group_chats
                            .non_whitelist_rate_per_minute,
                    )? {
                        config
                            .platforms
                            .qq
                            .group_chats
                            .non_whitelist_rate_per_minute = value;
                    }
                }
                13 if matches!(key, KeyCode::Enter) => {
                    select_platform_model_routes(stdout, config)?
                }
                14 if matches!(key, KeyCode::Enter) => select_platform_plugins(stdout, config)?,
                15 if matches!(key, KeyCode::Enter) => edit_qq_advanced(stdout, config)?,
                _ => {}
            },
            _ => {}
        }
    }
}

fn edit_u16_value(
    stdout: &mut io::Stdout,
    label: &'static str,
    current: u16,
) -> Result<Option<u16>> {
    let mut fields = vec![Field::new(label, current.to_string())];
    if !run_form(stdout, t(" EDIT VALUE ", " 编辑数值 "), &mut fields)? {
        return Ok(None);
    }
    match fields[0].value.trim().parse() {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            message(stdout, t("Invalid number.", "数值无效。"))?;
            Ok(None)
        }
    }
}

fn edit_u32_value(
    stdout: &mut io::Stdout,
    label: &'static str,
    current: u32,
) -> Result<Option<u32>> {
    let mut fields = vec![Field::new(label, current.to_string())];
    if !run_form(stdout, t(" EDIT VALUE ", " 编辑数值 "), &mut fields)? {
        return Ok(None);
    }
    match fields[0].value.trim().parse() {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            message(stdout, t("Invalid number.", "数值无效。"))?;
            Ok(None)
        }
    }
}

fn edit_qq_token(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    if let Some(value) = edit_inline_value(
        stdout,
        t(" REVERSE WEBSOCKET TOKEN ", " 反向 WebSocket 验证 Token "),
        &config.platforms.qq.access_token,
        true,
    )? {
        config.platforms.qq.access_token = value.trim().to_string();
    }
    Ok(())
}

fn parse_positive_id(value: &str) -> std::result::Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            t(
                "QQ/group id must be a positive integer.",
                "QQ 号/群号必须是正整数。",
            )
            .to_string()
        })
}

fn parse_id_lines(value: &str) -> std::result::Result<Vec<i64>, String> {
    let mut parsed = Vec::new();
    for (index, line) in value.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let id = parse_positive_id(line)
            .map_err(|error| format!("{} {}: {error}", t("Line", "第"), index + 1))?;
        if !parsed.contains(&id) {
            parsed.push(id);
        }
    }
    Ok(parsed)
}

fn prompt_single_id(
    stdout: &mut io::Stdout,
    item_label: &str,
    current: Option<i64>,
) -> Result<Option<i64>> {
    let action = if current.is_some() {
        t("Edit", "编辑")
    } else {
        t("Add", "新增")
    };
    let title = format!(" {action} {item_label} ");
    let Some(value) = edit_inline_value(
        stdout,
        &title,
        &current.map(|id| id.to_string()).unwrap_or_default(),
        false,
    )?
    else {
        return Ok(None);
    };
    match parse_positive_id(&value) {
        Ok(id) => Ok(Some(id)),
        Err(error) => {
            message(stdout, &error)?;
            Ok(None)
        }
    }
}

fn edit_qq_id_list(
    stdout: &mut io::Stdout,
    title: &'static str,
    item_label: &'static str,
    ids: &mut Vec<i64>,
) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let mut options = vec![
            t("+ Add one", "+ 新增一项").to_string(),
            t("+ Add multiple", "+ 批量新增").to_string(),
        ];
        options.extend(ids.iter().map(ToString::to_string));
        draw_menu(
            stdout,
            title,
            &options,
            selected,
            t(
                "[Enter]add/edit [Delete]remove [j/k]move [q]back",
                "[Enter]新增/编辑 [Delete]删除 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter if selected == 0 => {
                if let Some(id) = prompt_single_id(stdout, item_label, None)? {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            KeyCode::Enter if selected == 1 => {
                let mut value = String::new();
                loop {
                    edit_textarea(stdout, &mut value)?;
                    match parse_id_lines(&value) {
                        Ok(additions) => {
                            for id in additions {
                                if !ids.contains(&id) {
                                    ids.push(id);
                                }
                            }
                            break;
                        }
                        Err(error) => message(stdout, &error)?,
                    }
                }
            }
            KeyCode::Enter => {
                let index = selected - 2;
                if let Some(id) = prompt_single_id(stdout, item_label, ids.get(index).copied())? {
                    if ids
                        .iter()
                        .enumerate()
                        .any(|(other, item)| other != index && *item == id)
                    {
                        message(stdout, t("That id already exists.", "该号码已存在。"))?;
                    } else if let Some(item) = ids.get_mut(index) {
                        *item = id;
                    }
                }
            }
            KeyCode::Delete | KeyCode::Backspace if selected >= 2 => {
                ids.remove(selected - 2);
                selected = selected.min(ids.len() + 1);
            }
            _ => {}
        }
    }
}

fn parse_keyword_lines(value: &str) -> std::result::Result<Vec<String>, String> {
    let mut parsed = Vec::new();
    for (index, line) in value.lines().enumerate() {
        let keyword = line.trim();
        if keyword.is_empty() {
            continue;
        }
        if keyword.chars().count() > 128 || keyword.chars().any(char::is_control) {
            return Err(format!(
                "{} {}: {}",
                t("Line", "第"),
                index + 1,
                t("keyword is invalid or too long", "关键词无效或过长")
            ));
        }
        if !parsed.iter().any(|item| item == keyword) {
            parsed.push(keyword.to_string());
        }
    }
    Ok(parsed)
}

fn edit_keyword_list(stdout: &mut io::Stdout, keywords: &mut Vec<String>) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let mut options = vec![
            t("+ Add one", "+ 新增一项").to_string(),
            t("+ Add multiple", "+ 批量新增").to_string(),
        ];
        options.extend(keywords.iter().cloned());
        draw_menu(
            stdout,
            t(" GROUP WAKE KEYWORDS ", " 群聊触发关键词 "),
            &options,
            selected,
            t(
                "Keywords match only at the start of a message; @ always works",
                "关键词仅匹配消息开头；@ 始终有效",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter if selected == 0 => {
                if let Some(value) =
                    edit_inline_value(stdout, t(" ADD KEYWORD ", " 新增关键词 "), "", false)?
                {
                    match parse_keyword_lines(&value) {
                        Ok(additions) if additions.len() == 1 => {
                            let keyword = additions.into_iter().next().unwrap();
                            if !keywords.contains(&keyword) {
                                keywords.push(keyword);
                            }
                        }
                        _ => message(
                            stdout,
                            t("Enter exactly one valid keyword.", "请输入一个有效关键词。"),
                        )?,
                    }
                }
            }
            KeyCode::Enter if selected == 1 => {
                let mut value = String::new();
                loop {
                    edit_textarea(stdout, &mut value)?;
                    match parse_keyword_lines(&value) {
                        Ok(additions) => {
                            for keyword in additions {
                                if !keywords.contains(&keyword) {
                                    keywords.push(keyword);
                                }
                            }
                            break;
                        }
                        Err(error) => message(stdout, &error)?,
                    }
                }
            }
            KeyCode::Enter => {
                let index = selected - 2;
                if let Some(value) = edit_inline_value(
                    stdout,
                    t(" EDIT KEYWORD ", " 编辑关键词 "),
                    &keywords[index],
                    false,
                )? {
                    match parse_keyword_lines(&value) {
                        Ok(values) if values.len() == 1 => {
                            let value = values[0].clone();
                            if keywords
                                .iter()
                                .enumerate()
                                .any(|(other, item)| other != index && item == &value)
                            {
                                message(
                                    stdout,
                                    t("That keyword already exists.", "该关键词已存在。"),
                                )?;
                            } else {
                                keywords[index] = value;
                            }
                        }
                        _ => message(
                            stdout,
                            t("Enter exactly one valid keyword.", "请输入一个有效关键词。"),
                        )?,
                    }
                }
            }
            KeyCode::Delete | KeyCode::Backspace if selected >= 2 => {
                keywords.remove(selected - 2);
                selected = selected.min(keywords.len() + 1);
            }
            _ => {}
        }
    }
}

fn edit_qq_advanced(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let qq = &config.platforms.qq;
    let mut fields = vec![
        Field::new(
            t(
                "Asset base URL (empty = automatic)",
                "文件访问基础 URL（空 = 自动推导）",
            ),
            qq.asset_base_url.clone(),
        ),
        Field::new(
            t(
                "Max reply chars per message (0 = no split)",
                "单条回复最大字数（0 = 不分段）",
            ),
            qq.max_reply_chars.to_string(),
        ),
    ];
    if run_form(stdout, t(" QQ ADVANCED ", " QQ 高级设置 "), &mut fields)? {
        config.platforms.qq.asset_base_url =
            fields[0].value.trim().trim_end_matches('/').to_string();
        config.platforms.qq.max_reply_chars = fields[1].value.trim().parse().map_err(|_| {
            anyhow::anyhow!(t("Invalid maximum reply length.", "单条回复最大字数无效。"))
        })?;
    }
    Ok(())
}

fn select_platform_model_routes(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let mut options = Vec::with_capacity(config.platforms.qq.conversations.len() + 1);
        options.push(t("+ Add conversation", "+ 新增会话配置").to_string());
        options.extend(
            config
                .platforms
                .qq
                .conversations
                .iter()
                .map(platform_model_route_label),
        );
        selected = selected.min(options.len().saturating_sub(1));
        draw_menu(
            stdout,
            t(" QQ CONVERSATIONS ", " 私聊/群聊专属配置 "),
            &options,
            selected,
            t(
                "[Enter]add/edit [d]delete [j/k]move [q]back",
                "[Enter]新增/编辑 [d]删除 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(options.len().saturating_sub(1));
            }
            KeyCode::Enter if selected == 0 => edit_platform_model_route(stdout, config, None)?,
            KeyCode::Enter => edit_platform_model_route(stdout, config, Some(selected - 1))?,
            KeyCode::Char('d') | KeyCode::Delete if selected > 0 => {
                config.platforms.qq.conversations.remove(selected - 1);
                selected = selected.min(config.platforms.qq.conversations.len());
            }
            _ => {}
        }
    }
}

fn platform_model_route_label(route: &PlatformModelRoute) -> String {
    let kind = match route.conversation.kind {
        PlatformConversationKind::Private => t("private", "私聊"),
        PlatformConversationKind::Group => t("group", "群聊"),
    };
    let text_count = route.text_models.as_ref().map_or(0, Vec::len);
    let multimodal_count = route.multimodal_models.as_ref().map_or(0, Vec::len);
    let inherited = t("inherit", "继承");
    let text = if route.text_models.is_none() {
        inherited.to_string()
    } else {
        text_count.to_string()
    };
    let multimodal = if route.multimodal_models.is_none() {
        inherited.to_string()
    } else {
        multimodal_count.to_string()
    };
    let prompt = if route.extra_prompt.is_empty() {
        t("none", "无")
    } else {
        t("set", "已设置")
    };
    format!(
        "{kind} {} · {}:{text} {}:{multimodal} · {}:{prompt}",
        route.conversation.id,
        t("text", "文本"),
        t("media", "多模态"),
        t("prompt", "提示词")
    )
}

fn edit_platform_model_route(
    stdout: &mut io::Stdout,
    config: &mut AppConfig,
    route_index: Option<usize>,
) -> Result<()> {
    let mut route = route_index
        .and_then(|index| config.platforms.qq.conversations.get(index).cloned())
        .unwrap_or_else(|| PlatformModelRoute {
            conversation: PlatformConversationConfig {
                kind: PlatformConversationKind::Private,
                id: String::new(),
            },
            text_models: None,
            multimodal_models: None,
            extra_prompt: String::new(),
        });
    let mut selected = 0usize;
    loop {
        let kind_label = platform_conversation_kind_label(route.conversation.kind);
        let id_label = platform_conversation_id_label(route.conversation.kind);
        let options = [
            format!("{}: {}", t("Conversation type", "会话类型"), kind_label,),
            format!(
                "{id_label}: {}",
                if route.conversation.id.is_empty() {
                    t("not set", "未设置")
                } else {
                    route.conversation.id.as_str()
                },
            ),
            format!(
                "{}: {}",
                t("Text model pool", "文本模型池"),
                route_pool_summary(route.text_models.as_deref())
            ),
            format!(
                "{}: {}",
                t("Multimodal model pool", "多模态模型池"),
                route_pool_summary(route.multimodal_models.as_deref())
            ),
            format!(
                "{}: {}",
                t("Extra prompt", "额外提示词"),
                if route.extra_prompt.is_empty() {
                    t("none", "未设置")
                } else {
                    t("set", "已设置")
                }
            ),
            t("Save", "保存").to_string(),
            t("Cancel", "取消").to_string(),
        ];
        draw_menu(
            stdout,
            t(" EDIT QQ CONVERSATION ", " 编辑 QQ 会话配置 "),
            &options,
            selected,
            t(
                "Empty pools inherit the global configuration",
                "空模型池会继承全局配置",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => match selected {
                0 => select_platform_conversation_kind(stdout, &mut route.conversation.kind)?,
                1 => {
                    let title = format!(" {id_label} ");
                    if let Some(value) =
                        edit_inline_value(stdout, &title, &route.conversation.id, false)?
                    {
                        route.conversation.id = value.trim().to_string();
                    }
                }
                2 => select_platform_route_models(stdout, config, &mut route.text_models, false)?,
                3 => select_platform_route_models(
                    stdout,
                    config,
                    &mut route.multimodal_models,
                    true,
                )?,
                4 => edit_conversation_extra_prompt(stdout, &mut route.extra_prompt)?,
                5 => {
                    route.normalize();
                    if let Err(error) = config.validate_platform_model_route(&route) {
                        message(stdout, &error.to_string())?;
                        continue;
                    }
                    if config.platforms.qq.conversations.iter().enumerate().any(
                        |(index, existing)| {
                            Some(index) != route_index && existing.identity() == route.identity()
                        },
                    ) {
                        message(
                            stdout,
                            t(
                                "A configuration for this QQ conversation already exists.",
                                "该 QQ 会话的配置已存在。",
                            ),
                        )?;
                        continue;
                    }
                    match route_index {
                        Some(index) => config.platforms.qq.conversations[index] = route,
                        None => config.platforms.upsert_model_route(route),
                    }
                    return Ok(());
                }
                6 => return Ok(()),
                _ => {}
            },
            _ => {}
        }
    }
}

fn platform_conversation_kind_label(kind: PlatformConversationKind) -> &'static str {
    match kind {
        PlatformConversationKind::Private => t("Private chat", "私聊"),
        PlatformConversationKind::Group => t("Group chat", "群聊"),
    }
}

fn platform_conversation_id_label(kind: PlatformConversationKind) -> &'static str {
    match kind {
        PlatformConversationKind::Private => t("QQ id", "QQ 号"),
        PlatformConversationKind::Group => t("Group id", "群号"),
    }
}

fn select_platform_conversation_kind(
    stdout: &mut io::Stdout,
    kind: &mut PlatformConversationKind,
) -> Result<()> {
    let choices = [
        platform_conversation_kind_label(PlatformConversationKind::Private).to_string(),
        platform_conversation_kind_label(PlatformConversationKind::Group).to_string(),
    ];
    let current = platform_conversation_kind_label(*kind);
    let selected = select_choice(
        stdout,
        t("Conversation type", "会话类型"),
        current,
        &choices,
        "",
    )?;
    *kind = if selected == choices[1] {
        PlatformConversationKind::Group
    } else {
        PlatformConversationKind::Private
    };
    Ok(())
}

fn edit_conversation_extra_prompt(stdout: &mut io::Stdout, prompt: &mut String) -> Result<()> {
    edit_textarea(stdout, prompt)?;
    Ok(())
}

fn route_pool_summary(pool: Option<&[ActiveProviderModelConfig]>) -> String {
    match pool {
        None | Some([]) => t("inherit global", "继承全局").to_string(),
        Some(entries) if entries.len() == 1 => {
            format!("{} / {}", entries[0].provider_id, entries[0].model)
        }
        Some(entries) => format!("{} {}", entries.len(), t("models", "个模型")),
    }
}

fn select_platform_route_models(
    stdout: &mut io::Stdout,
    config: &AppConfig,
    pool: &mut Option<Vec<ActiveProviderModelConfig>>,
    multimodal: bool,
) -> Result<()> {
    let choices = if multimodal {
        config.multimodal_provider_model_choices()
    } else {
        config.text_provider_model_choices()
    };
    let mut selected = 0usize;
    loop {
        let mut options = Vec::with_capacity(choices.len() + 1);
        let inherit_marker = if pool.as_ref().is_none_or(Vec::is_empty) {
            "[*] "
        } else {
            "[ ] "
        };
        options.push(format!(
            "{inherit_marker}{}",
            t("Inherit global model pool", "继承全局模型池")
        ));
        options.extend(choices.iter().map(|choice| {
            let active = pool.as_ref().is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.provider_id == choice.provider_id && entry.model == choice.model
                })
            });
            let marker = if active { "[*] " } else { "[ ] " };
            format!("{marker}{}", choice.label())
        }));
        draw_menu(
            stdout,
            if multimodal {
                t(" SESSION MULTIMODAL MODELS ", " 会话多模态模型 ")
            } else {
                t(" SESSION TEXT MODELS ", " 会话文本模型 ")
            },
            &options,
            selected,
            t(
                "[Tab]add/remove [Enter/q]confirm",
                "[Tab]加入/移出 [Enter/q]确认",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Tab if selected == 0 => *pool = None,
            KeyCode::Tab => {
                let choice = &choices[selected - 1];
                let entries = pool.get_or_insert_with(Vec::new);
                if let Some(index) = entries.iter().position(|entry| {
                    entry.provider_id == choice.provider_id && entry.model == choice.model
                }) {
                    entries.remove(index);
                } else {
                    entries.push(ActiveProviderModelConfig {
                        provider_id: choice.provider_id.clone(),
                        model: choice.model.clone(),
                    });
                }
                if entries.is_empty() {
                    *pool = None;
                }
            }
            _ => {}
        }
    }
}

const REPLY_PROCESSOR_PLUGIN_ID: &str = "reply_processor";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ReplyProcessorSettingsForm {
    default_enabled: bool,
    threshold: usize,
    mode: String,
    followup_mention: bool,
    strip_period: bool,
    theme: String,
    max_height: u32,
    font_size: u32,
    code_font_size: u32,
    padding: u32,
    context_notice: bool,
    ttl_hours: u64,
    max_records: usize,
    send_tool_intercept: bool,
    font: String,
    title_font: String,
    code_font: String,
    emoji_font: String,
}

impl Default for ReplyProcessorSettingsForm {
    fn default() -> Self {
        Self {
            default_enabled: true,
            threshold: 300,
            mode: "image".to_string(),
            followup_mention: true,
            strip_period: true,
            theme: "paper".to_string(),
            max_height: 2600,
            font_size: 36,
            code_font_size: 30,
            padding: 64,
            context_notice: true,
            ttl_hours: 24,
            max_records: 3,
            send_tool_intercept: true,
            font: String::new(),
            title_font: String::new(),
            code_font: String::new(),
            emoji_font: String::new(),
        }
    }
}

fn select_platform_plugins(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let mut selected = 0usize;
    loop {
        let enabled = config
            .platforms
            .qq
            .plugins
            .get(REPLY_PROCESSOR_PLUGIN_ID)
            .map(|plugin| plugin.enabled_or(true))
            .unwrap_or(true);
        let state = if enabled {
            t("enabled", "已启用")
        } else {
            t("disabled", "未启用")
        };
        let options = [format!("{}: {state}", t("Reply processor", "回复处理"))];
        draw_menu(
            stdout,
            t(" TENCENT QQ PLUGINS ", " QQ 插件配置 "),
            &options,
            selected,
            t(
                "[Enter]configure [j/k]move [q]back",
                "[Enter]配置 [j/k]移动 [q]返回",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => edit_reply_processor(stdout, config)?,
            _ => {}
        }
    }
}

fn reply_processor_values(config: &AppConfig) -> Result<(bool, ReplyProcessorSettingsForm)> {
    let Some(instance) = config.platforms.qq.plugins.get(REPLY_PROCESSOR_PLUGIN_ID) else {
        return Ok((true, ReplyProcessorSettingsForm::default()));
    };
    let settings = serde_json::from_value(serde_json::Value::Object(instance.settings.clone()))?;
    Ok((instance.enabled_or(true), settings))
}

fn apply_reply_processor_values(
    config: &mut AppConfig,
    enabled: bool,
    settings: &ReplyProcessorSettingsForm,
) -> Result<()> {
    let serialized = serde_json::to_value(settings)?;
    let serde_json::Value::Object(known_settings) = serialized else {
        bail!("reply processor settings must serialize as an object");
    };
    let instance = config
        .platforms
        .qq
        .plugins
        .entry(REPLY_PROCESSOR_PLUGIN_ID.to_string())
        .or_default();
    instance.enabled = (!enabled).then_some(false);
    for (key, value) in known_settings {
        instance.settings.insert(key, value);
    }
    Ok(())
}

fn edit_reply_processor(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let (mut plugin_enabled, mut settings) = reply_processor_values(config)?;
    loop {
        let mut fields = vec![
            Field::boolean(t("Plugin enabled", "启用插件"), plugin_enabled),
            Field::boolean(
                t("Enabled for new conversations", "新会话默认启用"),
                settings.default_enabled,
            ),
            Field::new(
                t("Long reply threshold (characters)", "长回复阈值（字符）"),
                settings.threshold.to_string(),
            ),
            Field::new(t("Long reply mode", "长回复模式"), settings.mode.clone())
                .choices(&["image", "forward"]),
            Field::boolean(
                t("Mention sender after forwarding", "转发后艾特发起者"),
                settings.followup_mention,
            ),
            Field::boolean(
                t("Strip trailing Chinese period", "移除末尾中文句号"),
                settings.strip_period,
            ),
            Field::new(t("Image theme", "长图主题"), settings.theme.clone())
                .choices(&["paper", "light", "dark"]),
            Field::new(
                t("Image maximum height", "长图最大高度"),
                settings.max_height.to_string(),
            ),
            Field::new(
                t("Body font size", "正文字号"),
                settings.font_size.to_string(),
            ),
            Field::new(
                t("Code font size", "代码字号"),
                settings.code_font_size.to_string(),
            ),
            Field::new(t("Image padding", "长图边距"), settings.padding.to_string()),
            Field::boolean(
                t("Add image context notice", "注入长图上下文提示"),
                settings.context_notice,
            ),
            Field::new(
                t("Context notice TTL (hours)", "上下文提示保留小时"),
                settings.ttl_hours.to_string(),
            ),
            Field::new(
                t("Maximum context records", "上下文提示最大条数"),
                settings.max_records.to_string(),
            ),
            Field::boolean(
                t("Intercept send-message tool", "接管发送消息工具"),
                settings.send_tool_intercept,
            ),
            Field::new(
                t("Body font (empty = auto)", "正文字体（空 = 自动）"),
                settings.font.clone(),
            ),
            Field::new(
                t("Title font (empty = auto)", "标题字体（空 = 自动）"),
                settings.title_font.clone(),
            ),
            Field::new(
                t("Code font (empty = auto)", "代码字体（空 = 自动）"),
                settings.code_font.clone(),
            ),
            Field::new(
                t("Emoji font (empty = auto)", "Emoji 字体（空 = 自动）"),
                settings.emoji_font.clone(),
            ),
        ];
        if !run_form(stdout, t(" REPLY PROCESSOR ", " 回复处理 "), &mut fields)? {
            return Ok(());
        }
        plugin_enabled = parse_bool_field(&fields[0].value)?;
        settings = match parse_reply_processor_fields(&fields) {
            Ok(settings) => settings,
            Err(error) => {
                message(stdout, &error)?;
                continue;
            }
        };
        apply_reply_processor_values(config, plugin_enabled, &settings)?;
        return Ok(());
    }
}

fn parse_reply_processor_fields(
    fields: &[Field],
) -> std::result::Result<ReplyProcessorSettingsForm, String> {
    let bool_at =
        |index: usize| parse_bool_field(&fields[index].value).map_err(|error| error.to_string());
    let settings = ReplyProcessorSettingsForm {
        default_enabled: bool_at(1)?,
        threshold: parse_reply_processor_value(fields, 2, t("threshold", "阈值"))?,
        mode: fields[3].value.trim().to_string(),
        followup_mention: bool_at(4)?,
        strip_period: bool_at(5)?,
        theme: fields[6].value.trim().to_string(),
        max_height: parse_reply_processor_value(fields, 7, t("maximum height", "最大高度"))?,
        font_size: parse_reply_processor_value(fields, 8, t("font size", "字号"))?,
        code_font_size: parse_reply_processor_value(fields, 9, t("code font size", "代码字号"))?,
        padding: parse_reply_processor_value(fields, 10, t("padding", "边距"))?,
        context_notice: bool_at(11)?,
        ttl_hours: parse_reply_processor_value(fields, 12, "TTL")?,
        max_records: parse_reply_processor_value(fields, 13, t("maximum records", "最大条数"))?,
        send_tool_intercept: bool_at(14)?,
        font: fields[15].value.trim().to_string(),
        title_font: fields[16].value.trim().to_string(),
        code_font: fields[17].value.trim().to_string(),
        emoji_font: fields[18].value.trim().to_string(),
    };
    validate_reply_processor_settings(&settings)?;
    Ok(settings)
}

fn parse_reply_processor_value<T>(
    fields: &[Field],
    index: usize,
    label: &str,
) -> std::result::Result<T, String>
where
    T: std::str::FromStr,
{
    fields[index]
        .value
        .trim()
        .parse()
        .map_err(|_| format!("{}: {label}", t("Invalid value", "无效值")))
}

fn validate_reply_processor_settings(
    settings: &ReplyProcessorSettingsForm,
) -> std::result::Result<(), String> {
    if settings.threshold == 0 || settings.threshold > 100_000 {
        return Err(t(
            "Threshold must be between 1 and 100000.",
            "阈值必须在 1 到 100000 之间。",
        )
        .to_string());
    }
    if !matches!(settings.mode.as_str(), "image" | "forward") {
        return Err(t(
            "Mode must be image or forward.",
            "模式必须是 image 或 forward。",
        )
        .to_string());
    }
    if !matches!(settings.theme.as_str(), "paper" | "light" | "dark") {
        return Err(t(
            "Theme must be paper, light, or dark.",
            "主题必须是 paper、light 或 dark。",
        )
        .to_string());
    }
    if !(1000..=5000).contains(&settings.max_height) {
        return Err(t(
            "Image maximum height must be between 1000 and 5000.",
            "长图最大高度必须在 1000 到 5000 之间。",
        )
        .to_string());
    }
    if !(24..=56).contains(&settings.font_size) || !(20..=46).contains(&settings.code_font_size) {
        return Err(t(
            "Body font size must be 24-56 and code font size must be 20-46.",
            "正文字号必须为 24-56，代码字号必须为 20-46。",
        )
        .to_string());
    }
    if !(36..=120).contains(&settings.padding) {
        return Err(t(
            "Image padding must be between 36 and 120.",
            "长图边距必须在 36 到 120 之间。",
        )
        .to_string());
    }
    if !(1..=168).contains(&settings.ttl_hours) || !(1..=10).contains(&settings.max_records) {
        return Err(t(
            "Context TTL must be 1-168 hours and maximum records must be 1-10.",
            "上下文保留时间必须为 1-168 小时，最大条数必须为 1-10。",
        )
        .to_string());
    }
    Ok(())
}

fn format_id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_id_list(value: &str) -> Result<Vec<i64>> {
    value
        .split([',', ' ', '\u{3000}', ';'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let id = item.parse::<i64>().map_err(|_| {
                anyhow::anyhow!(t("invalid id: {}", "无效的号码：{}").replace("{}", item))
            })?;
            if id <= 0 {
                bail!(t(
                    "QQ and group ids must be positive",
                    "QQ 号和群号必须为正数"
                ));
            }
            Ok(id)
        })
        .collect()
}

fn edit_provider_form(
    stdout: &mut io::Stdout,
    provider: ProviderConfig,
) -> Result<Option<ProviderConfig>> {
    let current_context_window = provider
        .model_context_window
        .get(&provider.default_model)
        .copied()
        .unwrap_or_default();

    // 将 extra_body 格式化为 JSON 字符串，方便编辑
    let extra_body_string = provider
        .extra_body
        .as_ref()
        .and_then(|v| serde_json::to_string_pretty(v).ok())
        .unwrap_or_default();

    let mut fields = vec![
        Field::new(t("Configuration ID", "配置 ID"), provider.id.clone()),
        Field::new(t("Display name", "显示名称"), provider.display_name.clone()),
        Field::new("Base URL", provider.base_url.clone()),
        Field::new(t("Protocol", "协议"), provider.protocol.clone()).choices(&[
            "auto",
            "openai-chat",
            "openai-responses",
            "anthropic",
        ]),
        Field::new(
            t("API Key or $env:NAME", "API Key 或 $env:NAME"),
            provider.api_key.clone().unwrap_or_default(),
        )
        .sensitive(),
        Field::new(
            t("Current model", "当前模型"),
            provider.default_model.clone(),
        ),
        Field::new(
            t(
                "Model context window (tokens, 0=auto)",
                "模型上下文窗口 (tokens, 0=自动)",
            ),
            current_context_window.to_string(),
        ),
        Field::new(
            t("Timeout (seconds)", "超时秒数"),
            provider.timeout_seconds.to_string(),
        ),
        Field::new("Temperature", provider.temperature.to_string()),
        Field::textarea(
            t("Extra request body (JSON)", "额外请求体 (JSON)"),
            extra_body_string,
        ),
    ];

    // 循环直到用户取消或输入合法 JSON 对象
    loop {
        if !run_form(stdout, t(" EDIT PROVIDER ", " 编辑供应商 "), &mut fields)? {
            return Ok(None);
        }

        // 提取各个字段的值（索引保持不变）
        let default_model = fields[5].value.trim().to_string();
        let model_context_window_raw = fields[6].value.trim().parse::<usize>().unwrap_or_default();
        let timeout = fields[7].value.trim().parse().unwrap_or(60);
        let temperature = fields[8].value.trim().parse().unwrap_or(0.7);

        let extra_body = match parse_extra_body(&fields[9].value) {
            Ok(extra_body) => extra_body,
            Err(error) => {
                message(stdout, &error)?;
                continue;
            }
        };

        // 构建 model_context_window
        let mut model_context_window = provider.model_context_window.clone();
        match model_context_window_raw {
            0 => {
                model_context_window.remove(&default_model);
            }
            value => {
                model_context_window.insert(default_model.clone(), value);
            }
        }

        let mut models = provider.models.clone();
        if !default_model.trim().is_empty() && !models.iter().any(|item| item == &default_model) {
            models.push(default_model.clone());
        }

        // 所有验证通过，返回新的 ProviderConfig
        return Ok(Some(ProviderConfig {
            id: fields[0].value.trim().to_string(),
            display_name: fields[1].value.trim().to_string(),
            base_url: normalize_base_url(&fields[2].value),
            protocol: fields[3].value.trim().to_string(),
            api_key: Some(fields[4].value.trim().to_string()).filter(|value| !value.is_empty()),
            models,
            model_context_window,
            model_modalities: provider.model_modalities.clone(),
            default_model,
            timeout_seconds: timeout,
            temperature,
            anthropic_max_tokens: provider.anthropic_max_tokens,
            extra_body,
        }));
    }
}

fn parse_extra_body(
    value: &str,
) -> std::result::Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Object(object)) => Ok(Some(object)),
        Ok(_) => Err(t(
            "The extra request body must be a JSON object (for example {\"key\": \"value\"})",
            "额外请求体必须是 JSON 对象 (如 {\"key\": \"value\"})",
        )
        .to_string()),
        Err(error) => Err(if is_zh() {
            format!("无效 JSON: {error}")
        } else {
            format!("Invalid JSON: {error}")
        }),
    }
}

fn edit_model_form(
    stdout: &mut io::Stdout,
    provider: &mut ProviderConfig,
    model: &str,
) -> Result<bool> {
    let context_window = provider
        .model_context_window
        .get(model)
        .copied()
        .unwrap_or_default();
    let mut fields = vec![
        Field::modalities(
            t("Supported input", "支持输入"),
            modality_field_value(provider, model),
        ),
        Field::new(
            t(
                "Model context window (tokens, 0=auto)",
                "模型上下文窗口 (tokens, 0=自动)",
            ),
            context_window.to_string(),
        ),
        Field::new("Temperature", provider.temperature.to_string()),
    ];
    if !run_form(stdout, t(" EDIT MODEL ", " 编辑模型 "), &mut fields)? {
        return Ok(false);
    }
    provider
        .model_modalities
        .insert(model.to_string(), parse_modalities(&fields[0].value));
    match fields[1].value.trim().parse::<usize>().unwrap_or_default() {
        0 => {
            provider.model_context_window.remove(model);
        }
        value => {
            provider
                .model_context_window
                .insert(model.to_string(), value);
        }
    }
    provider.temperature = fields[2].value.trim().parse().unwrap_or(0.7);
    Ok(true)
}

fn edit_settings(stdout: &mut io::Stdout, config: &mut AppConfig) -> Result<()> {
    let language = language_choice_value(&config.display.language).unwrap_or("auto");
    let mut fields = vec![
        Field::boolean(t("Enable tools", "工具启用"), config.tools.enabled),
        Field::new(
            t("Maximum tool rounds", "工具最大轮数"),
            config.tools.max_rounds.to_string(),
        ),
        Field::new(
            t("Tool loading mode", "工具加载模式"),
            config.tools.loading_mode.clone(),
        )
        .choices(&["full", "hybrid"]),
        Field::boolean(
            t("Remember loaded tools", "记住已加载工具"),
            config.tools.persist_loaded_tools,
        ),
        Field::boolean(t("Enable skills", "Skills 启用"), config.skills.enabled),
        Field::boolean(
            t("Allow command execution", "允许执行命令"),
            config.skills.allow_command_execution,
        ),
        Field::new(t("Interface language", "界面语言"), language.to_string())
            .choices(&["auto", "en", "zh"]),
        Field::new(
            t("Show reasoning", "显示思考过程"),
            config.display.reasoning.clone(),
        )
        .choices(&["summary", "full", "hidden"]),
        Field::new(
            t("Show tool call details", "显示工具调用信息"),
            config.display.tool_calls.clone(),
        )
        .choices(&["summary", "full", "hidden"]),
        Field::new(
            t("Command output lines", "命令输出显示行数"),
            config.display.command_output_lines.to_string(),
        ),
        Field::boolean(
            t("Readable tool names", "工具名可读显示"),
            config.display.readable_tool_names,
        ),
        Field::boolean(
            t(
                "Show token usage in shell conversations",
                "Shell 无缝对话显示 Token 计数",
            ),
            config.display.show_token_usage,
        ),
        Field::new(
            t(
                "Show current provider/model in Mixed mode",
                "Mixed 时显示本次供应商/模型",
            ),
            parse_mixed_endpoint_display(&config.display.mixed_model_endpoint_display),
        )
        .choices(&["off", "interactive", "all"]),
        Field::new(
            t("When context reaches its limit", "上下文到达上限后"),
            config.context.on_overflow.clone(),
        )
        .choices(&["compact", "pop"]),
    ];
    run_form_without_buttons(stdout, t(" GLOBAL SETTINGS ", " 全局设置 "), &mut fields)?;
    config.tools.enabled = parse_bool_field(&fields[0].value)?;
    config.tools.max_rounds = fields[1].value.trim().parse::<usize>()?;
    config.tools.loading_mode = normalize_tools_loading_mode(&fields[2].value);
    config.tools.persist_loaded_tools = parse_bool_field(&fields[3].value)?;
    config.skills.enabled = parse_bool_field(&fields[4].value)?;
    config.skills.allow_command_execution = parse_bool_field(&fields[5].value)?;
    config.display.language = language_choice_value(&fields[6].value)
        .unwrap_or("auto")
        .to_string();
    config.display.reasoning = fields[7].value.trim().to_string();
    config.display.tool_calls = fields[8].value.trim().to_string();
    config.display.command_output_lines = fields[9]
        .value
        .trim()
        .parse::<usize>()?
        .min(MAX_COMMAND_OUTPUT_LINES);
    config.display.readable_tool_names = parse_bool_field(&fields[10].value)?;
    config.display.show_token_usage = parse_bool_field(&fields[11].value)?;
    config.display.mixed_model_endpoint_display = parse_mixed_endpoint_display(&fields[12].value);
    config.context.on_overflow = fields[13].value.trim().to_string();
    Ok(())
}

fn language_choice_label(value: &str, zh: bool) -> Option<&'static str> {
    match (value.trim(), zh) {
        ("auto", false) => Some("Auto"),
        ("auto", true) => Some("自动"),
        ("en", false) => Some("English"),
        ("en", true) => Some("英语"),
        ("zh", false) => Some("Simplified Chinese"),
        ("zh", true) => Some("简体中文"),
        _ => None,
    }
}

fn language_choice_value(value: &str) -> Option<&'static str> {
    match value.trim() {
        "auto" | "Auto" | "自动" => Some("auto"),
        "en" | "English" | "英语" => Some("en"),
        "zh" | "Simplified Chinese" | "简体中文" => Some("zh"),
        _ => None,
    }
}

fn parse_mixed_endpoint_display(value: &str) -> String {
    match value.trim() {
        "关" | "Off" | "off" => "off".to_string(),
        "全部模式" | "All modes" | "all" => "all".to_string(),
        _ => "interactive".to_string(),
    }
}

fn normalize_tools_loading_mode(value: &str) -> String {
    match value.trim() {
        "lazy" => "hybrid".to_string(),
        value => value.to_string(),
    }
}

fn parse_bool_field(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "1" | "on" | "启用" | "是" => Ok(true),
        "false" | "no" | "n" | "0" | "off" | "禁用" | "否" => Ok(false),
        value => {
            if is_zh() {
                bail!("无效的布尔值: {value}")
            } else {
                bail!("Invalid boolean value: {value}")
            }
        }
    }
}

struct FcitxState {
    last_state: Option<char>,
}

impl FcitxState {
    fn new() -> Self {
        let last_state = fcitx5_state();
        run_fcitx5_remote("-c");
        Self { last_state }
    }

    fn enter_editing(&mut self) {
        if self.last_state == Some('2') {
            run_fcitx5_remote("-o");
        }
    }

    fn leave_editing(&mut self) {
        self.last_state = fcitx5_state();
        run_fcitx5_remote("-c");
    }
}

fn fcitx5_state() -> Option<char> {
    let output = Command::new("fcitx5-remote").output().ok()?;
    output.stdout.first().copied().map(char::from)
}

fn run_fcitx5_remote(arg: &str) {
    let _ = Command::new("fcitx5-remote").arg(arg).spawn();
}

fn edit_inline_value(
    stdout: &mut io::Stdout,
    title: &str,
    current: &str,
    sensitive: bool,
) -> Result<Option<String>> {
    let mut value = current.to_string();
    let mut cursor = value.chars().count();
    let mut fcitx = FcitxState::new();
    fcitx.enter_editing();
    loop {
        draw_inline_editor(stdout, title, &value, cursor, sensitive)?;
        match read_key()? {
            KeyCode::Esc => {
                fcitx.leave_editing();
                execute!(stdout, Hide)?;
                return Ok(None);
            }
            KeyCode::Enter => {
                fcitx.leave_editing();
                execute!(stdout, Hide)?;
                return Ok(Some(value));
            }
            KeyCode::Left => cursor = cursor.saturating_sub(1),
            KeyCode::Right => cursor = (cursor + 1).min(value.chars().count()),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = value.chars().count(),
            KeyCode::Backspace if cursor > 0 => remove_char_before_cursor(&mut value, &mut cursor),
            KeyCode::Delete => remove_char_at_cursor(&mut value, cursor),
            KeyCode::Char(ch) => insert_char_at_cursor(&mut value, &mut cursor, ch),
            _ => {}
        }
    }
}

fn draw_inline_editor(
    stdout: &mut io::Stdout,
    title: &str,
    value: &str,
    cursor: usize,
    sensitive: bool,
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let width = 72_u16.min(cols.saturating_sub(2)).max(12);
    let height = 6_u16.min(rows.max(6));
    let x = cols.saturating_sub(width) / 2;
    let y = rows.saturating_sub(height) / 2;
    let capacity = width.saturating_sub(4) as usize;
    let chars = value.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let start = cursor
        .saturating_sub(capacity.saturating_sub(1))
        .min(chars.len().saturating_sub(capacity));
    let end = (start + capacity).min(chars.len());
    let visible = if sensitive {
        "*".repeat(end.saturating_sub(start))
    } else {
        chars[start..end].iter().collect::<String>()
    };

    queue!(stdout, Hide, Clear(ClearType::All))?;
    draw_box(stdout, x, y, width, height, title)?;
    queue!(
        stdout,
        MoveTo(x + 2, y + 2),
        Print(pad(&visible, capacity)),
        MoveTo(x + 2, y + 4),
        SetAttribute(Attribute::Dim),
        Print(truncate(
            t("[Enter]save  [Esc]cancel", "[Enter]保存  [Esc]取消"),
            capacity,
        )),
        SetAttribute(Attribute::Reset),
        MoveTo(
            x + 2 + u16::try_from(cursor.saturating_sub(start)).unwrap_or(u16::MAX),
            y + 2,
        ),
        Show,
    )?;
    stdout.flush()?;
    Ok(())
}

fn run_form(stdout: &mut io::Stdout, title: &str, fields: &mut [Field]) -> Result<bool> {
    let mut selected = 0usize;
    let mut editing = false;
    let mut fcitx = FcitxState::new();
    let mut cursors = fields
        .iter()
        .map(|field| field.value.chars().count())
        .collect::<Vec<_>>();
    loop {
        draw_form(stdout, title, fields, selected, editing, &cursors, true)?;
        match read_key()? {
            KeyCode::Esc if editing => {
                fcitx.leave_editing();
                editing = false;
            }
            KeyCode::Esc | KeyCode::Char('q') if !editing => return Ok(false),
            KeyCode::Enter if editing => {
                fcitx.leave_editing();
                editing = false;
            }
            KeyCode::Enter if !editing && selected == fields.len() => return Ok(true),
            KeyCode::Enter if !editing && selected == fields.len() + 1 => return Ok(false),
            KeyCode::Enter if !editing && fields[selected].boolean => {
                let value = select_bool(
                    stdout,
                    fields[selected].label,
                    parse_bool_field(&fields[selected].value)?,
                )?;
                fields[selected].value = value.to_string();
                cursors[selected] = fields[selected].value.chars().count();
            }
            KeyCode::Enter if !editing && fields[selected].modalities => {
                fields[selected].value = select_multi_choice(
                    stdout,
                    fields[selected].label,
                    &fields[selected].value,
                    &["text", "image", "audio", "video", "pdf"]
                        .iter()
                        .map(|item| item.to_string())
                        .collect::<Vec<_>>(),
                )?;
                cursors[selected] = fields[selected].value.chars().count();
            }
            KeyCode::Enter if !editing && !fields[selected].choices.is_empty() => {
                fields[selected].value = select_choice(
                    stdout,
                    fields[selected].label,
                    &fields[selected].value,
                    &fields[selected].choices,
                    fields[selected].empty_choice_label,
                )?;
                cursors[selected] = fields[selected].value.chars().count();
            }
            KeyCode::Enter if !editing && fields[selected].textarea => {
                edit_textarea(stdout, &mut fields[selected].value)?;
                cursors[selected] = fields[selected].value.chars().count();
                if !fields[selected].sensitive {
                    return Ok(true);
                }
            }
            KeyCode::Enter if !editing => {
                if !fields[selected].boolean {
                    fcitx.enter_editing();
                    editing = true;
                }
            }
            KeyCode::Char('s') if !editing => return Ok(true),
            KeyCode::Up | KeyCode::Char('k') if !editing => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if !editing => {
                selected = (selected + 1).min(fields.len() + 1)
            }
            KeyCode::Left | KeyCode::Char('h') if !editing && selected == fields.len() + 1 => {
                selected = fields.len()
            }
            KeyCode::Right | KeyCode::Char('l') if !editing && selected == fields.len() => {
                selected = fields.len() + 1
            }
            KeyCode::Left if editing => cursors[selected] = cursors[selected].saturating_sub(1),
            KeyCode::Right if editing => {
                cursors[selected] =
                    (cursors[selected] + 1).min(fields[selected].value.chars().count())
            }
            KeyCode::Home if editing => cursors[selected] = 0,
            KeyCode::End if editing => cursors[selected] = fields[selected].value.chars().count(),
            KeyCode::Backspace if editing => {
                if cursors[selected] > 0 {
                    remove_char_before_cursor(&mut fields[selected].value, &mut cursors[selected]);
                }
            }
            KeyCode::Delete if editing => {
                remove_char_at_cursor(&mut fields[selected].value, cursors[selected])
            }
            KeyCode::Char(char) if editing => {
                insert_char_at_cursor(&mut fields[selected].value, &mut cursors[selected], char)
            }
            _ => {}
        }
    }
}

fn run_form_without_buttons(
    stdout: &mut io::Stdout,
    title: &str,
    fields: &mut [Field],
) -> Result<()> {
    let mut selected = 0usize;
    let mut editing = false;
    let mut fcitx = FcitxState::new();
    let mut cursors = fields
        .iter()
        .map(|field| field.value.chars().count())
        .collect::<Vec<_>>();
    loop {
        draw_form(stdout, title, fields, selected, editing, &cursors, false)?;
        match read_key()? {
            KeyCode::Esc if editing => {
                fcitx.leave_editing();
                editing = false;
            }
            KeyCode::Esc | KeyCode::Char('q') if !editing => return Ok(()),
            KeyCode::Enter if editing => {
                fcitx.leave_editing();
                editing = false;
            }
            KeyCode::Enter if !editing && fields[selected].boolean => {
                let value = select_bool(
                    stdout,
                    fields[selected].label,
                    parse_bool_field(&fields[selected].value)?,
                )?;
                fields[selected].value = value.to_string();
                cursors[selected] = fields[selected].value.chars().count();
            }
            KeyCode::Enter if !editing && fields[selected].modalities => {
                fields[selected].value = select_multi_choice(
                    stdout,
                    fields[selected].label,
                    &fields[selected].value,
                    &["text", "image", "audio", "video", "pdf"]
                        .iter()
                        .map(|item| item.to_string())
                        .collect::<Vec<_>>(),
                )?;
                cursors[selected] = fields[selected].value.chars().count();
            }
            KeyCode::Enter if !editing && !fields[selected].choices.is_empty() => {
                fields[selected].value = select_choice(
                    stdout,
                    fields[selected].label,
                    &fields[selected].value,
                    &fields[selected].choices,
                    fields[selected].empty_choice_label,
                )?;
                cursors[selected] = fields[selected].value.chars().count();
            }
            KeyCode::Enter if !editing && fields[selected].textarea => {
                edit_textarea(stdout, &mut fields[selected].value)?;
                cursors[selected] = fields[selected].value.chars().count();
                if !fields[selected].sensitive {
                    return Ok(());
                }
            }
            KeyCode::Enter if !editing => {
                if !fields[selected].boolean {
                    fcitx.enter_editing();
                    editing = true;
                }
            }
            KeyCode::Up | KeyCode::Char('k') if !editing => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if !editing => {
                selected = (selected + 1).min(fields.len().saturating_sub(1))
            }
            KeyCode::Left if editing => cursors[selected] = cursors[selected].saturating_sub(1),
            KeyCode::Right if editing => {
                cursors[selected] =
                    (cursors[selected] + 1).min(fields[selected].value.chars().count())
            }
            KeyCode::Home if editing => cursors[selected] = 0,
            KeyCode::End if editing => cursors[selected] = fields[selected].value.chars().count(),
            KeyCode::Backspace if editing => {
                if cursors[selected] > 0 {
                    remove_char_before_cursor(&mut fields[selected].value, &mut cursors[selected]);
                }
            }
            KeyCode::Delete if editing => {
                remove_char_at_cursor(&mut fields[selected].value, cursors[selected])
            }
            KeyCode::Char(char) if editing => {
                insert_char_at_cursor(&mut fields[selected].value, &mut cursors[selected], char)
            }
            _ => {}
        }
    }
}

fn select_bool(stdout: &mut io::Stdout, label: &str, current: bool) -> Result<bool> {
    let mut selected = if current { 0 } else { 1 };
    let options = [
        boolean_label(true).to_string(),
        boolean_label(false).to_string(),
    ];
    loop {
        draw_menu(stdout, label, &options, selected, "")?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(current),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Enter => return Ok(selected == 0),
            _ => {}
        }
    }
}

fn select_choice(
    stdout: &mut io::Stdout,
    label: &str,
    current: &str,
    choices: &[String],
    empty_label: &'static str,
) -> Result<String> {
    let mut selected = choices.iter().position(|item| item == current).unwrap_or(0);
    loop {
        let options = choices
            .iter()
            .map(|choice| choice_label(choice, empty_label))
            .collect::<Vec<_>>();
        draw_menu(stdout, label, &options, selected, "")?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(current.to_string()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(choices.len() - 1),
            KeyCode::Enter => return Ok(choices[selected].clone()),
            _ => {}
        }
    }
}

fn select_multi_choice(
    stdout: &mut io::Stdout,
    label: &str,
    current: &str,
    choices: &[String],
) -> Result<String> {
    let mut selected = 0usize;
    let mut active = choices
        .iter()
        .map(|choice| has_modality(current, choice))
        .collect::<Vec<_>>();
    loop {
        let options = choices
            .iter()
            .zip(&active)
            .map(|(choice, active)| {
                format!(
                    "{} {}",
                    if *active { "[*]" } else { "[ ]" },
                    choice_label(choice, "")
                )
            })
            .collect::<Vec<_>>();
        draw_menu(
            stdout,
            label,
            &options,
            selected,
            t(
                "[Tab]select/deselect [Enter/q]confirm",
                "[Tab]选择/取消 [Enter/q]确认",
            ),
        )?;
        match read_key()? {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                return Ok(choices
                    .iter()
                    .zip(active)
                    .filter_map(|(choice, active)| active.then(|| choice.clone()))
                    .collect::<Vec<_>>()
                    .join(", "))
            }
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(choices.len() - 1),
            KeyCode::Tab | KeyCode::Char(' ') => active[selected] = !active[selected],
            _ => {}
        }
    }
}

fn choice_label(choice: &str, empty_label: &str) -> String {
    if choice.is_empty() {
        empty_label.to_string()
    } else if let Some((provider, model)) = choice.split_once('\t') {
        format!("{provider} / {model}")
    } else if let Some(label) = localized_choice_label(choice, is_zh()) {
        label.to_string()
    } else {
        choice.to_string()
    }
}

fn boolean_label(value: bool) -> &'static str {
    if value {
        t("Enabled", "启用")
    } else {
        t("Disabled", "禁用")
    }
}

fn localized_choice_label(value: &str, zh: bool) -> Option<&'static str> {
    if let Some(label) = language_choice_label(value, zh) {
        return Some(label);
    }
    match (value.trim(), zh) {
        ("minimal", false) => Some("Minimal"),
        ("minimal", true) => Some("最低"),
        ("low", false) => Some("Low"),
        ("low", true) => Some("低"),
        ("medium", false) => Some("Medium"),
        ("medium", true) => Some("中"),
        ("high", false) => Some("High"),
        ("high", true) => Some("高"),
        ("xhigh", false) => Some("Extra high"),
        ("xhigh", true) => Some("极高"),
        ("global", false) => Some("Global"),
        ("global", true) => Some("全球"),
        ("mainland", false) => Some("Mainland China"),
        ("mainland", true) => Some("中国大陆"),
        ("summary", false) => Some("Summary"),
        ("summary", true) => Some("摘要"),
        ("full", false) => Some("Full"),
        ("full", true) => Some("完整"),
        ("hidden", false) => Some("Hidden"),
        ("hidden", true) => Some("隐藏"),
        ("hybrid", false) => Some("Hybrid"),
        ("hybrid", true) => Some("混合"),
        ("off", false) => Some("Off"),
        ("off", true) => Some("关"),
        ("interactive", false) => Some("Interactive only"),
        ("interactive", true) => Some("仅交互模式"),
        ("all", false) => Some("All modes"),
        ("all", true) => Some("全部模式"),
        ("pop", false) => Some("Remove oldest"),
        ("pop", true) => Some("弹出旧消息"),
        ("compact", false) => Some("Compact context"),
        ("compact", true) => Some("压缩上下文"),
        ("text", false) => Some("Text"),
        ("text", true) => Some("文本"),
        ("image", false) => Some("Image"),
        ("image", true) => Some("图片"),
        ("audio", false) => Some("Audio"),
        ("audio", true) => Some("音频"),
        ("video", false) => Some("Video"),
        ("video", true) => Some("视频"),
        ("pdf", false) => Some("PDF"),
        ("pdf", true) => Some("PDF"),
        ("自动", false) => Some("Auto"),
        ("自动", true) => Some("自动"),
        _ => None,
    }
}

fn provider_model_choice_values(config: &AppConfig, include_current: bool) -> Vec<String> {
    let mut choices = vec![String::new()];
    if include_current {
        choices.push(format!(
            "{OPENCODE_PROVIDER_ID}\t{OPENCODE_DEFAULT_VISION_MODEL}"
        ));
    }
    choices.extend(
        config
            .provider_model_choices()
            .into_iter()
            .map(|choice| choice.value()),
    );
    choices
}

fn vision_provider_model_choice_values(config: &AppConfig) -> Vec<String> {
    let mut choices = vec![
        String::new(),
        format!("{OPENCODE_PROVIDER_ID}\t{OPENCODE_DEFAULT_VISION_MODEL}"),
    ];
    choices.extend(
        config
            .multimodal_provider_model_choices()
            .into_iter()
            .map(|choice| choice.value()),
    );
    choices.sort();
    choices.dedup();
    choices
}

fn active_multimodal_label(config: &AppConfig) -> String {
    let choices = config.active_multimodal_provider_model_choices();
    if choices.is_empty() {
        format!(
            "{} / {}",
            OPENCODE_PROVIDER_ID, OPENCODE_DEFAULT_VISION_MODEL
        )
    } else if choices.len() == 1 {
        choices[0].label()
    } else {
        t("Mixed", "混合").to_string()
    }
}

fn modality_field_value(provider: &ProviderConfig, model: &str) -> String {
    provider
        .input_modalities(model)
        .unwrap_or_else(|| vec!["text".to_string()])
        .join(", ")
}

fn parse_modalities(value: &str) -> Vec<String> {
    value
        .split(|ch| ch == ',' || ch == '，' || ch == '\n' || ch == '\r')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn has_modality(value: &str, modality: &str) -> bool {
    parse_modalities(value).iter().any(|item| item == modality)
}

fn select_active_multimodal_provider(
    stdout: &mut io::Stdout,
    config: &mut AppConfig,
) -> Result<()> {
    let choices = config.multimodal_provider_model_choices();
    if choices.is_empty() {
        message(
            stdout,
            t(
                "No models support image input. Configure Supported input under Edit model first.",
                "没有支持图片输入的模型，请先在编辑模型里配置支持输入。",
            ),
        )?;
        return Ok(());
    }
    let mut selected = choices
        .iter()
        .position(|choice| {
            config.is_active_multimodal_provider_model(&choice.provider_id, &choice.model)
        })
        .unwrap_or(0);
    loop {
        let options = choices
            .iter()
            .map(|choice| {
                let marker = if config
                    .is_active_multimodal_provider_model(&choice.provider_id, &choice.model)
                {
                    "[*] "
                } else {
                    "[ ] "
                };
                format!("{marker}{}", choice.label())
            })
            .collect::<Vec<_>>();
        draw_menu(
            stdout,
            t(" SELECT MULTIMODAL MODEL ", " 选择多模态模型 "),
            &options,
            selected,
            t(
                "[Tab]activate/deactivate [Enter/q]confirm",
                "[Tab]激活/取消 [Enter/q]确认",
            ),
        )?;
        match read_key()? {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(options.len() - 1),
            KeyCode::Tab => {
                let choice = choices[selected].clone();
                config
                    .toggle_active_multimodal_provider_model(&choice.provider_id, &choice.model)?;
            }
            _ => {}
        }
    }
}

fn vision_provider_value(config: &AppConfig) -> String {
    let vision = &config.plugins.vision;
    if vision.vision_provider_id.trim().is_empty() {
        format!("{OPENCODE_PROVIDER_ID}\t{OPENCODE_DEFAULT_VISION_MODEL}")
    } else if vision.vision_model.trim().is_empty() {
        config
            .provider(Some(vision.vision_provider_id.trim()))
            .map(|provider| format!("{}\t{}", provider.id, provider.default_model))
            .unwrap_or_else(|_| vision.vision_provider_id.clone())
    } else {
        format!("{}\t{}", vision.vision_provider_id, vision.vision_model)
    }
}

fn kb_embedding_provider_value(config: &AppConfig) -> String {
    let kb = &config.plugins.knowledge_base;
    if kb.embedding_provider_id.trim().is_empty() {
        String::new()
    } else if kb.embedding_model.trim().is_empty() {
        config
            .provider(Some(kb.embedding_provider_id.trim()))
            .map(|provider| format!("{}\t{}", provider.id, provider.default_model))
            .unwrap_or_else(|_| kb.embedding_provider_id.clone())
    } else {
        format!("{}\t{}", kb.embedding_provider_id, kb.embedding_model)
    }
}

fn parse_provider_model_choice(value: &str) -> (String, String) {
    let value = value.trim();
    if value.is_empty() {
        return (String::new(), String::new());
    }
    if let Some((provider, model)) = value.split_once('\t') {
        return (provider.trim().to_string(), model.trim().to_string());
    }
    (value.to_string(), String::new())
}

fn edit_textarea(stdout: &mut io::Stdout, value: &mut String) -> Result<()> {
    execute!(
        stdout,
        Show,
        LeaveAlternateScreen,
        Clear(ClearType::All),
        MoveTo(0, 0)
    )?;
    stdout.flush()?;
    terminal::disable_raw_mode()?;
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(value.as_bytes())?;
    let path = file.path().to_path_buf();
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let status = Command::new(&editor)
        .arg(&path)
        .status()
        .or_else(|_| Command::new("nano").arg(&path).status());
    if let Err(err) = status {
        if is_zh() {
            eprintln!("无法打开编辑器: {err}");
        } else {
            eprintln!("Failed to open editor: {err}");
        }
    }
    *value = std::fs::read_to_string(&path)?.trim().to_string();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Clear(ClearType::All), Hide)?;
    Ok(())
}

fn draw_menu(
    stdout: &mut io::Stdout,
    title: &str,
    options: &[String],
    selected: usize,
    status: &str,
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let content_w = options
        .iter()
        .map(|option| option.chars().count())
        .max()
        .unwrap_or(20)
        .max(title.chars().count())
        .max(menu_help(status).chars().count())
        + 6;
    let width = (content_w as u16).min(cols.saturating_sub(4)).max(56);
    let height = (options.len() as u16 + 5)
        .min(rows.saturating_sub(2))
        .max(7);
    let x = cols.saturating_sub(width) / 2;
    let y = rows.saturating_sub(height) / 2;

    queue!(stdout, Clear(ClearType::All))?;
    draw_box(stdout, x, y, width, height, title)?;
    queue!(
        stdout,
        MoveTo(x + 2, y + height - 1),
        SetAttribute(Attribute::Dim),
        Print(truncate(
            menu_help(status),
            width.saturating_sub(4) as usize
        )),
        SetAttribute(Attribute::Reset)
    )?;
    for (index, option) in options.iter().enumerate() {
        queue!(stdout, MoveTo(x + 2, y + index as u16 + 2))?;
        if index == selected {
            queue!(
                stdout,
                SetAttribute(Attribute::Reverse),
                Print(pad(option, width.saturating_sub(4) as usize)),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(stdout, Print(pad(option, width.saturating_sub(4) as usize)))?;
        }
    }
    stdout.flush()?;
    Ok(())
}

fn menu_help(status: &str) -> &str {
    if status.is_empty() {
        t(
            "[j/k]move [Enter]select [q]back",
            "[j/k]移动 [Enter]选择 [q]返回",
        )
    } else {
        status
    }
}

fn draw_box(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    title: &str,
) -> Result<()> {
    queue!(
        stdout,
        MoveTo(x, y),
        Print(format!(
            "┌{}┐",
            "─".repeat(width.saturating_sub(2) as usize)
        ))
    )?;
    for row in 1..height.saturating_sub(1) {
        queue!(
            stdout,
            MoveTo(x, y + row),
            Print(format!(
                "│{}│",
                " ".repeat(width.saturating_sub(2) as usize)
            ))
        )?;
    }
    queue!(
        stdout,
        MoveTo(x, y + height.saturating_sub(1)),
        Print(format!(
            "└{}┘",
            "─".repeat(width.saturating_sub(2) as usize)
        ))
    )?;
    queue!(
        stdout,
        MoveTo(x + 2, y),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn draw_column(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    title: &str,
    items: &[String],
    selected: usize,
    scroll: usize,
    active: bool,
) -> Result<()> {
    let attr = if active {
        Attribute::Reverse
    } else {
        Attribute::Bold
    };
    queue!(
        stdout,
        MoveTo(x, y),
        SetAttribute(attr),
        Print(pad(&truncate(title, width as usize), width as usize)),
        SetAttribute(Attribute::Reset)
    )?;
    let visible_rows = height.saturating_sub(2) as usize;
    let start = column_scroll(selected, scroll, visible_rows);
    for row in 0..visible_rows {
        let index = start + row;
        if index >= items.len() {
            break;
        }
        queue!(stdout, MoveTo(x, y + row as u16 + 1))?;
        let line = truncate(&items[index], width as usize);
        if index == selected {
            queue!(
                stdout,
                SetAttribute(Attribute::Reverse),
                Print(pad(&line, width as usize)),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(stdout, Print(pad(&line, width as usize)))?;
        }
    }
    Ok(())
}

fn column_visible_rows() -> usize {
    terminal::size()
        .map(|(_, rows)| rows.saturating_sub(4) as usize)
        .unwrap_or(1)
}

fn column_scroll(selected: usize, scroll: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    if selected < scroll {
        selected
    } else if selected >= scroll + visible_rows {
        selected + 1 - visible_rows
    } else {
        scroll
    }
}

fn draw_form(
    stdout: &mut io::Stdout,
    title: &str,
    fields: &[Field],
    selected: usize,
    editing: bool,
    cursors: &[usize],
    show_buttons: bool,
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let width = cols.saturating_sub(8).min(96).max(48);
    let height = (fields.len() as u16 + 8)
        .min(rows.saturating_sub(4))
        .max(10);
    let x = cols.saturating_sub(width) / 2;
    let y = rows.saturating_sub(height) / 2;
    queue!(stdout, Clear(ClearType::All))?;
    draw_box(stdout, x, y, width, height, title)?;
    queue!(
        stdout,
        MoveTo(x + 2, y + 1),
        Print(if show_buttons {
            t(
                "[j/k]move [Enter]edit/open editor [s]confirm [q]back",
                "[j/k]移动 [Enter]编辑/打开编辑器 [s]确认 [q]返回",
            )
        } else {
            t(
                "[j/k]move [Enter]edit/open editor [q]back",
                "[j/k]移动 [Enter]编辑/打开编辑器 [q]返回",
            )
        })
    )?;
    let mut cursor = None;
    for (index, field) in fields.iter().enumerate() {
        let row_y = y + index as u16 + 3;
        queue!(stdout, MoveTo(x + 2, row_y))?;
        let marker = if index == selected { ">" } else { " " };
        let value = field_display_value(field, index == selected && editing);
        let prefix = format!("{marker} {}: ", field.label);
        let line = truncate(
            &format!("{prefix}{value}"),
            width.saturating_sub(4) as usize,
        );
        if index == selected && !editing {
            queue!(
                stdout,
                SetAttribute(Attribute::Reverse),
                Print(pad(&line, width.saturating_sub(4) as usize)),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(stdout, Print(pad(&line, width.saturating_sub(4) as usize)))?;
        }
        if index == selected && editing {
            let cursor_text = take_chars(&field.value.replace('\n', " "), cursors[index]);
            let cursor_x = x
                + 2
                + display_width(&prefix) as u16
                + display_width(&truncate(&cursor_text, width.saturating_sub(4) as usize)) as u16;
            cursor = Some((cursor_x.min(x + width.saturating_sub(3)), row_y));
        }
    }
    if show_buttons {
        let button_y = y + fields.len() as u16 + 4;
        draw_form_button(
            stdout,
            x + 2,
            button_y,
            t(" Save ", " 保存 "),
            selected == fields.len() && !editing,
        )?;
        draw_form_button(
            stdout,
            x + 14,
            button_y,
            t(" Back ", " 返回 "),
            selected == fields.len() + 1 && !editing,
        )?;
    }

    let mode = if editing {
        t(
            "Editing; Enter/Esc finishes editing",
            "编辑中，Enter/Esc 结束编辑",
        )
    } else if show_buttons {
        t(
            "Navigating; Enter selects the current item",
            "导航中，Enter 选择当前项",
        )
    } else {
        t(
            "Navigating; Enter selects the current item; [q]back",
            "导航中，Enter 选择当前项，[q]返回",
        )
    };
    queue!(
        stdout,
        MoveTo(x + 2, y + height.saturating_sub(1)),
        Print(truncate(mode, width.saturating_sub(4) as usize))
    )?;
    if let Some((x, y)) = cursor {
        queue!(stdout, Show, MoveTo(x, y))?;
    } else {
        queue!(stdout, Hide)?;
    }
    stdout.flush()?;
    Ok(())
}

fn field_display_value(field: &Field, reveal_sensitive: bool) -> String {
    if field.textarea && field.value.is_empty() {
        t("(Enter opens $EDITOR)", "(Enter 打开 $EDITOR)").to_string()
    } else if field.sensitive && !field.value.is_empty() && !reveal_sensitive {
        if field.textarea {
            if is_zh() {
                format!("[已配置 {} 项]", parse_key_list(&field.value).len())
            } else {
                format!("[{} configured]", parse_key_list(&field.value).len())
            }
        } else {
            "********".to_string()
        }
    } else if !field.choices.is_empty() && field.value.is_empty() {
        field.empty_choice_label.to_string()
    } else if !field.choices.is_empty() {
        choice_label(&field.value, field.empty_choice_label)
    } else if field.boolean {
        match parse_bool_field(&field.value) {
            Ok(value) => boolean_label(value).to_string(),
            Err(_) => field.value.clone(),
        }
    } else if field.modalities {
        parse_modalities(&field.value)
            .iter()
            .map(|value| choice_label(value, ""))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        truncate(&field.value.replace('\n', " "), 70)
    }
}

fn draw_form_button(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    label: &str,
    selected: bool,
) -> Result<()> {
    queue!(stdout, MoveTo(x, y))?;
    if selected {
        queue!(
            stdout,
            SetAttribute(Attribute::Reverse),
            Print(label),
            SetAttribute(Attribute::Reset)
        )?;
    } else {
        queue!(stdout, Print(label))?;
    }
    Ok(())
}

fn insert_char_at_cursor(value: &mut String, cursor: &mut usize, ch: char) {
    let byte_index = byte_index_for_char(value, *cursor);
    value.insert(byte_index, ch);
    *cursor += 1;
}

fn remove_char_before_cursor(value: &mut String, cursor: &mut usize) {
    let end = byte_index_for_char(value, *cursor);
    let start = byte_index_for_char(value, cursor.saturating_sub(1));
    value.replace_range(start..end, "");
    *cursor -= 1;
}

fn remove_char_at_cursor(value: &mut String, cursor: usize) {
    if cursor >= value.chars().count() {
        return;
    }
    let start = byte_index_for_char(value, cursor);
    let end = byte_index_for_char(value, cursor + 1);
    value.replace_range(start..end, "");
}

fn byte_index_for_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn take_chars(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn message(stdout: &mut io::Stdout, text: &str) -> Result<()> {
    queue!(
        stdout,
        Clear(ClearType::All),
        MoveTo(0, 0),
        Print(text),
        MoveTo(0, 2),
        Print(t("Press any key to continue", "按任意键继续"))
    )?;
    stdout.flush()?;
    let _ = read_key()?;
    Ok(())
}

fn read_key() -> Result<KeyCode> {
    read_key_with_timeout(None).map(|key| key.expect("blocking read should return a key"))
}

fn read_key_with_timeout(timeout: Option<Duration>) -> Result<Option<KeyCode>> {
    loop {
        if let Some(timeout) = timeout {
            if !event::poll(timeout)? {
                return Ok(None);
            }
        }
        if let Event::Key(KeyEvent { code, .. }) = event::read()? {
            return Ok(Some(code));
        }
    }
}

fn active_label(config: &AppConfig) -> String {
    match config.active_provider_model_choices().as_slice() {
        [] => t("Not configured", "未配置").to_string(),
        [choice] => format!("{} / {}", choice.provider_name, choice.model),
        _ => t("Mixed", "混合").to_string(),
    }
}

fn normalize_base_url(value: &str) -> String {
    let mut url = value.trim().trim_end_matches('/').to_string();
    if url.ends_with("/chat/completions") {
        url.truncate(url.len() - "/chat/completions".len());
    }
    url
}

fn truncate(value: &str, max: usize) -> String {
    if display_width(value) <= max {
        return value.to_string();
    }
    let mut width = 0usize;
    let mut output = String::new();
    let ellipsis_width = 1usize;
    for ch in value.chars() {
        let char_width = display_width(&ch.to_string());
        if width + char_width + ellipsis_width > max {
            break;
        }
        output.push(ch);
        width += char_width;
    }
    output.push('…');
    output
}

fn display_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| match ch {
            '\u{1100}'..='\u{115F}'
            | '\u{2329}'..='\u{232A}'
            | '\u{2E80}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE10}'..='\u{FE19}'
            | '\u{FE30}'..='\u{FE6F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}' => 2,
            _ => 1,
        })
        .sum()
}

fn pad(value: &str, width: usize) -> String {
    let value = truncate(value, width);
    let len = display_width(&value);
    if len >= width {
        value
    } else {
        format!("{value}{}", " ".repeat(width - len))
    }
}

struct Field {
    label: &'static str,
    value: String,
    textarea: bool,
    sensitive: bool,
    boolean: bool,
    modalities: bool,
    choices: Vec<String>,
    empty_choice_label: &'static str,
}

impl Field {
    fn new(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            textarea: false,
            sensitive: false,
            boolean: false,
            modalities: false,
            choices: Vec::new(),
            empty_choice_label: t("Use current provider", "使用当前 Provider"),
        }
    }

    fn boolean(label: &'static str, value: bool) -> Self {
        Self {
            label,
            value: value.to_string(),
            textarea: false,
            sensitive: false,
            boolean: true,
            modalities: false,
            choices: Vec::new(),
            empty_choice_label: t("Use current provider", "使用当前 Provider"),
        }
    }

    fn textarea(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            textarea: true,
            sensitive: false,
            boolean: false,
            modalities: false,
            choices: Vec::new(),
            empty_choice_label: t("Use current provider", "使用当前 Provider"),
        }
    }

    fn choices(mut self, choices: &[&str]) -> Self {
        self.choices = choices.iter().map(|item| item.to_string()).collect();
        self
    }

    fn sensitive(mut self) -> Self {
        self.sensitive = true;
        self
    }

    fn modalities(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            textarea: false,
            sensitive: false,
            boolean: false,
            modalities: true,
            choices: Vec::new(),
            empty_choice_label: t("Use current provider", "使用当前 Provider"),
        }
    }

    fn choices_owned(mut self, choices: Vec<String>) -> Self {
        self.choices = choices;
        self
    }

    fn empty_choice_label(mut self, label: &'static str) -> Self {
        self.empty_choice_label = label;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_reply_processor_values, field_display_value, language_choice_label,
        language_choice_value, parse_extra_body, parse_id_lines, parse_id_list,
        parse_keyword_lines, platform_conversation_id_label, platform_conversation_kind_label,
        reply_processor_values, route_pool_summary, t, validate_reply_processor_settings,
        vision_provider_model_choice_values, Field, ReplyProcessorSettingsForm,
        REPLY_PROCESSOR_PLUGIN_ID,
    };
    use crate::config::{AppConfig, PlatformConversationKind, PlatformPluginInstanceConfig};

    #[test]
    fn sensitive_field_is_masked_until_actively_edited() {
        let field = Field::new("API Key", "secret-key".to_string()).sensitive();

        assert_eq!(field_display_value(&field, false), "********");
        assert_eq!(field_display_value(&field, true), "secret-key");
    }

    #[test]
    fn empty_sensitive_field_remains_empty() {
        let field = Field::new("API Key", String::new()).sensitive();

        assert_eq!(field_display_value(&field, false), "");
    }

    #[test]
    fn sensitive_textarea_displays_configured_item_count() {
        let field = Field::textarea("API Keys", "first\n\nsecond, third".to_string()).sensitive();

        assert_eq!(
            field_display_value(&field, false),
            t("[3 configured]", "[已配置 3 项]")
        );
    }

    #[test]
    fn empty_sensitive_textarea_keeps_editor_placeholder() {
        let field = Field::textarea("API Keys", String::new()).sensitive();

        assert_eq!(
            field_display_value(&field, false),
            t("(Enter opens $EDITOR)", "(Enter 打开 $EDITOR)")
        );
    }

    #[test]
    fn language_choices_have_locale_specific_labels() {
        assert_eq!(language_choice_label("auto", false), Some("Auto"));
        assert_eq!(language_choice_label("en", false), Some("English"));
        assert_eq!(
            language_choice_label("zh", false),
            Some("Simplified Chinese")
        );
        assert_eq!(language_choice_label("auto", true), Some("自动"));
        assert_eq!(language_choice_label("en", true), Some("英语"));
        assert_eq!(language_choice_label("zh", true), Some("简体中文"));
    }

    #[test]
    fn language_choice_labels_map_to_stable_values() {
        for value in ["auto", "Auto", "自动"] {
            assert_eq!(language_choice_value(value), Some("auto"));
        }
        for value in ["en", "English", "英语"] {
            assert_eq!(language_choice_value(value), Some("en"));
        }
        for value in ["zh", "Simplified Chinese", "简体中文"] {
            assert_eq!(language_choice_value(value), Some("zh"));
        }
        assert_eq!(language_choice_value("unsupported"), None);
    }

    #[test]
    fn extra_body_parser_accepts_only_json_objects() {
        for input in ["true", "\"hello\"", "[1, 2, 3]", "{invalid"] {
            assert!(parse_extra_body(input).is_err());
        }

        let parsed = parse_extra_body(r#"{"enable_thinking":false}"#)
            .unwrap()
            .unwrap();
        assert_eq!(parsed["enable_thinking"], false);
        assert!(parse_extra_body("  ").unwrap().is_none());
    }

    #[test]
    fn reply_processor_defaults_match_platform_contract() {
        let config = AppConfig::default();
        let (enabled, settings) = reply_processor_values(&config).unwrap();

        assert!(enabled);
        assert!(settings.default_enabled);
        assert_eq!(settings.threshold, 300);
        assert_eq!(settings.mode, "image");
        assert!(settings.followup_mention);
        assert!(settings.strip_period);
        assert_eq!(settings.theme, "paper");
        assert_eq!(settings.max_height, 2600);
        assert_eq!(settings.font_size, 36);
        assert_eq!(settings.code_font_size, 30);
        assert_eq!(settings.padding, 64);
        assert!(settings.context_notice);
        assert_eq!(settings.ttl_hours, 24);
        assert_eq!(settings.max_records, 3);
        assert!(settings.send_tool_intercept);
        assert!(settings.font.is_empty());
        assert!(settings.title_font.is_empty());
        assert!(settings.code_font.is_empty());
        assert!(settings.emoji_font.is_empty());
    }

    #[test]
    fn reply_processor_settings_use_generic_map_and_preserve_unknown_keys() {
        let mut config = AppConfig::default();
        let mut instance = PlatformPluginInstanceConfig {
            enabled: Some(false),
            ..PlatformPluginInstanceConfig::default()
        };
        instance
            .settings
            .insert("future_option".to_string(), serde_json::json!({"value": 1}));
        config
            .platforms
            .qq
            .plugins
            .insert(REPLY_PROCESSOR_PLUGIN_ID.to_string(), instance);
        let settings = ReplyProcessorSettingsForm {
            threshold: 512,
            mode: "forward".to_string(),
            ..ReplyProcessorSettingsForm::default()
        };

        apply_reply_processor_values(&mut config, true, &settings).unwrap();

        let instance = &config.platforms.qq.plugins[REPLY_PROCESSOR_PLUGIN_ID];
        assert_eq!(instance.enabled, None);
        assert_eq!(instance.settings["threshold"], 512);
        assert_eq!(instance.settings["mode"], "forward");
        assert_eq!(instance.settings["future_option"]["value"], 1);
        let (enabled, reparsed) = reply_processor_values(&config).unwrap();
        assert!(enabled);
        assert_eq!(reparsed, settings);
    }

    #[test]
    fn reply_processor_range_validation_rejects_unsafe_render_settings() {
        assert!(validate_reply_processor_settings(&ReplyProcessorSettingsForm::default()).is_ok());
        assert!(
            validate_reply_processor_settings(&ReplyProcessorSettingsForm {
                threshold: 0,
                ..ReplyProcessorSettingsForm::default()
            })
            .is_err()
        );
        assert!(
            validate_reply_processor_settings(&ReplyProcessorSettingsForm {
                max_height: 999,
                ..ReplyProcessorSettingsForm::default()
            })
            .is_err()
        );
        assert!(
            validate_reply_processor_settings(&ReplyProcessorSettingsForm {
                ttl_hours: 169,
                ..ReplyProcessorSettingsForm::default()
            })
            .is_err()
        );
    }

    #[test]
    fn route_pool_and_id_helpers_express_inheritance_and_positive_ids() {
        assert_eq!(route_pool_summary(None), t("inherit global", "继承全局"));
        assert_eq!(
            route_pool_summary(Some(&[])),
            t("inherit global", "继承全局")
        );
        assert_eq!(parse_id_list("123, 456").unwrap(), vec![123, 456]);
        assert!(parse_id_list("0").is_err());
        assert!(parse_id_list("-1").is_err());
        assert_eq!(parse_id_lines("123\n456\n123\n").unwrap(), vec![123, 456]);
        assert!(parse_id_lines("123\ninvalid\n456").is_err());
        assert_eq!(
            parse_keyword_lines("Miyu\n 小羽 \nMiyu").unwrap(),
            vec!["Miyu", "小羽"]
        );
    }

    #[test]
    fn qq_batch_inputs_are_line_based_trimmed_and_deduplicated() {
        assert_eq!(
            parse_id_lines(" 123 \r\n\r\n456\n123\n").unwrap(),
            vec![123, 456]
        );
        assert!(parse_id_lines("123,456").is_err());
        assert_eq!(
            parse_keyword_lines(" Miyu \r\n\r\n小羽\nMiyu\n").unwrap(),
            vec!["Miyu", "小羽"]
        );
    }

    #[test]
    fn qq_conversation_labels_are_localized_and_id_label_tracks_type() {
        assert_eq!(
            platform_conversation_kind_label(PlatformConversationKind::Private),
            t("Private chat", "私聊")
        );
        assert_eq!(
            platform_conversation_kind_label(PlatformConversationKind::Group),
            t("Group chat", "群聊")
        );
        assert_eq!(
            platform_conversation_id_label(PlatformConversationKind::Private),
            t("QQ id", "QQ 号")
        );
        assert_eq!(
            platform_conversation_id_label(PlatformConversationKind::Group),
            t("Group id", "群号")
        );
    }

    #[test]
    fn explicit_vision_choices_only_include_image_capable_models() {
        let mut config = AppConfig::default();
        let provider = &mut config.providers[0];
        provider.models = vec!["text-only".to_string(), "vision".to_string()];
        provider
            .model_modalities
            .insert("text-only".to_string(), vec!["text".to_string()]);
        provider.model_modalities.insert(
            "vision".to_string(),
            vec!["text".to_string(), "image".to_string()],
        );
        let provider_id = provider.id.clone();

        let choices = vision_provider_model_choice_values(&config);

        assert!(choices.contains(&format!("{provider_id}\tvision")));
        assert!(!choices.contains(&format!("{provider_id}\ttext-only")));
    }
}
