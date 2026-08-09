//! Catching the deletions a hallucinating model would regret.
//!
//! The model is told to prefer `trash_path` and to list files before deleting
//! them, but those are sentences in a prompt — nothing enforces them. This
//! module is the enforcement, and only for the case that cannot be undone:
//! `rm` and its relatives, and `apply_patch`'s `*** Delete File:`. Anything
//! that lands in the system Trash is already recoverable and is left alone.
//!
//! This is not a shell parser and does not try to be one. It recognises the
//! handful of shapes a model actually writes, and when it cannot recognise
//! something that smells like a deletion it asks rather than guessing.

use std::path::{Path, PathBuf};

/// How many entries a preview walk will visit before giving up on an exact
/// count. Past this the number stops being decision-relevant anyway.
const PREVIEW_LIMIT: usize = 1000;

/// Where a deletion request came from — decides what the confirmation can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A shell command, with the original text for the "show me the command" view.
    Shell(String),
    /// `apply_patch` deleting files it already resolved.
    ApplyPatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing here worth interrupting the user for.
    Allow,
    /// A fuse: refused regardless of settings, with the reason to show.
    Refuse(String),
    /// Ask the user, showing this.
    Ask(Preview),
}

/// What the confirmation shows. Every number here comes from Miyu walking the
/// filesystem, never from the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    /// Paths about to be destroyed. Empty when the command was not understood.
    pub targets: Vec<PathBuf>,
    /// Files only. Directories are structure, not the thing a person is
    /// counting when they read "21 files".
    pub files: usize,
    pub entries: usize,
    /// True when `entries` hit the walk limit and is a floor, not an exact count.
    pub truncated: bool,
    pub bytes: u64,
    /// False when the command matched a deletion word but its targets could not
    /// be resolved — the user gets a plainly worded "I could not read this".
    pub understood: bool,
    pub source: Source,
}

impl Preview {
    /// Offering "move to Trash instead" only makes sense when we know what to
    /// move. With an unparsed command we would be guessing at the user's files.
    pub fn can_divert_to_trash(&self) -> bool {
        self.understood && !self.targets.is_empty()
    }
}

/// Commands whose whole purpose is to destroy what they name.
const DELETING_COMMANDS: &[&str] = &["rm", "shred", "unlink", "rmdir"];

/// Words that mean a segment might delete even though we could not resolve it.
/// Matched against the argv, never as a raw substring of the command — that is
/// how the old `ensure_readonly_command` ended up rejecting `grep -rn "a->b"`.
const SUSPICIOUS: &[&str] = &["rm", "shred", "unlink", "rmdir", "-delete", "truncate"];

/// Judges one shell command.
pub fn assess_command(command: &str) -> Verdict {
    let mut targets: Vec<PathBuf> = Vec::new();
    let mut suspicious = false;

    for segment in split_segments(command) {
        let argv = tokenize(&segment);
        if argv.is_empty() {
            continue;
        }
        // An empty variable is the classic one-shot disaster: `rm -rf "$D"/`
        // with `D` unset becomes `rm -rf /`. There is no legitimate form of
        // this, so it never reaches a confirmation.
        if let Some(reason) = empty_expansion_hazard(&argv) {
            return Verdict::Refuse(reason);
        }
        match segment_targets(&argv) {
            // Recognised: we know exactly what it does, destructive or not.
            Some(found) => targets.extend(found),
            // Not a shape we read. If it mentions deleting at all, say so
            // rather than pretend it is safe.
            None => {
                if argv.iter().any(|word| SUSPICIOUS.contains(&word.as_str())) {
                    suspicious = true;
                }
            }
        }
        if let Some(path) = truncating_redirect(&segment) {
            targets.push(path);
        }
    }

    for target in &targets {
        if let Some(reason) = fuse(target) {
            return Verdict::Refuse(reason);
        }
    }

    // Scratch directories exist to be filled and emptied. Asking about them is
    // pure noise, and the model writes and removes temp files constantly.
    targets.retain(|target| !is_scratch(target));

    if targets.is_empty() {
        if suspicious {
            return Verdict::Ask(Preview {
                targets: Vec::new(),
                files: 0,
                entries: 0,
                truncated: false,
                bytes: 0,
                understood: false,
                source: Source::Shell(command.to_string()),
            });
        }
        return Verdict::Allow;
    }

    Verdict::Ask(preview_of(targets, Source::Shell(command.to_string())))
}

/// Judges a set of paths `apply_patch` is about to unlink.
pub fn assess_paths(targets: Vec<PathBuf>) -> Verdict {
    for target in &targets {
        if let Some(reason) = fuse(target) {
            return Verdict::Refuse(reason);
        }
    }
    let targets: Vec<PathBuf> = targets
        .into_iter()
        .filter(|target| !is_scratch(target))
        .collect();
    if targets.is_empty() {
        return Verdict::Allow;
    }
    Verdict::Ask(preview_of(targets, Source::ApplyPatch))
}

fn preview_of(targets: Vec<PathBuf>, source: Source) -> Preview {
    let mut entries = 0usize;
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut truncated = false;
    for target in &targets {
        let walked = walk(target, PREVIEW_LIMIT - entries.min(PREVIEW_LIMIT));
        entries += walked.entries;
        files += walked.files;
        bytes += walked.bytes;
        truncated |= walked.truncated;
    }
    Preview {
        targets,
        files,
        entries,
        truncated,
        bytes,
        understood: true,
        source,
    }
}

/// Counts what a path actually contains. This is the number the user decides
/// on, and it is why the guard is worth having: a model that believes it is
/// removing three build artifacts will show up here asking to remove 4000.
#[derive(Default)]
struct Walked {
    entries: usize,
    files: usize,
    bytes: u64,
    truncated: bool,
}

fn walk(path: &Path, budget: usize) -> Walked {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Walked::default();
    };
    if !meta.is_dir() {
        return Walked {
            entries: 1,
            files: 1,
            bytes: meta.len(),
            truncated: false,
        };
    }
    let mut walked = Walked {
        entries: 1,
        ..Walked::default()
    };
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if walked.entries >= budget {
            walked.truncated = true;
            return walked;
        }
        let Ok(children) = std::fs::read_dir(&dir) else {
            continue;
        };
        for child in children.flatten() {
            if walked.entries >= budget {
                walked.truncated = true;
                return walked;
            }
            walked.entries += 1;
            match child.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(child.path()),
                Ok(_) => {
                    walked.files += 1;
                    walked.bytes += child.metadata().map(|meta| meta.len()).unwrap_or(0);
                }
                Err(_) => {}
            }
        }
    }
    walked
}

/// The two paths that are never a deliberate target, only ever an accident.
fn fuse(target: &Path) -> Option<String> {
    let home = home_dir();
    if target == Path::new("/") {
        return Some(
            crate::i18n::text(
                "this would delete the whole filesystem",
                "这会删掉整个文件系统",
            )
            .to_string(),
        );
    }
    if home.as_deref() == Some(target) {
        return Some(
            crate::i18n::text(
                "this would delete your entire home directory",
                "这会删掉你的整个家目录",
            )
            .to_string(),
        );
    }
    None
}

fn is_scratch(target: &Path) -> bool {
    let mut roots = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
    if let Some(dir) = std::env::var_os("TMPDIR") {
        roots.push(PathBuf::from(dir));
    }
    roots
        .iter()
        .any(|root| target != root.as_path() && target.starts_with(root))
}

/// `rm -rf "$MISSING"/x` collapses to `rm -rf /x` once the shell expands it.
/// We cannot expand variables ourselves, so we look for the shape instead: a
/// path argument whose variable reference is the only thing standing between
/// the command and the filesystem root.
fn empty_expansion_hazard(argv: &[String]) -> Option<String> {
    if !argv
        .iter()
        .any(|word| DELETING_COMMANDS.contains(&word.as_str()))
    {
        return None;
    }
    for word in argv.iter().skip(1) {
        let Some(rest) = strip_leading_variable(word) else {
            continue;
        };
        // What remains after the variable. If it is nothing or just slashes,
        // an empty expansion makes this the root.
        if rest.is_empty() || rest.chars().all(|character| character == '/') {
            return Some(
                crate::i18n::text(
                    "this deletes a path built from a variable; if the variable is empty it becomes the filesystem root",
                    "这条命令要删的路径来自一个变量；变量为空时它会变成文件系统根目录",
                )
                .to_string(),
            );
        }
    }
    None
}

/// Returns what follows a leading `$VAR` / `${VAR}`, or `None` when the word
/// does not start with a variable reference.
fn strip_leading_variable(word: &str) -> Option<&str> {
    let rest = word.strip_prefix('$')?;
    if let Some(rest) = rest.strip_prefix('{') {
        return rest.split_once('}').map(|(_, tail)| tail);
    }
    let end = rest
        .find(|character: char| !character.is_alphanumeric() && character != '_')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(&rest[end..])
}

/// What a segment does, as far as we can tell.
///
/// `Some(paths)` means the program was recognised and this is everything it
/// destroys — an empty vector is a positive "recognised, harmless", which is
/// why this is not just `Vec<PathBuf>`: `truncate -s 10M` grows a file and
/// must not be confused with a command we failed to read.
fn segment_targets(argv: &[String]) -> Option<Vec<PathBuf>> {
    let program = argv.first()?;
    let program = program.rsplit('/').next().unwrap_or(program);

    if DELETING_COMMANDS.contains(&program) {
        return Some(operands(&argv[1..]));
    }
    if program == "truncate" {
        // Only `-s 0` destroys; other sizes may be growing a file.
        let zeroing = argv
            .windows(2)
            .any(|pair| pair[0] == "-s" && (pair[1] == "0" || pair[1] == "0B"));
        return Some(if zeroing {
            operands(&argv[1..])
        } else {
            Vec::new()
        });
    }
    if program == "find" {
        let deletes = argv.iter().any(|word| word == "-delete")
            || argv
                .windows(2)
                .any(|pair| pair[0] == "-exec" && pair[1] == "rm");
        // The roots `find` was pointed at: leading operands before the first
        // predicate. Whatever it matches lives underneath them.
        return Some(if deletes {
            operands_until_flag(&argv[1..])
        } else {
            Vec::new()
        });
    }
    None
}

fn operands(words: &[String]) -> Vec<PathBuf> {
    words
        .iter()
        .filter(|word| !word.starts_with('-'))
        .map(|word| expand(word))
        .collect()
}

fn operands_until_flag(words: &[String]) -> Vec<PathBuf> {
    words
        .iter()
        .take_while(|word| !word.starts_with('-'))
        .map(|word| expand(word))
        .collect()
}

/// `cmd > file` only destroys when `file` already holds something. Treating a
/// bare `>` as destructive would flag every `cargo build > log.txt`.
fn truncating_redirect(segment: &str) -> Option<PathBuf> {
    let mut quote: Option<char> = None;
    for (index, character) in segment.char_indices() {
        match character {
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            '>' if quote.is_none() => {
                // `>>` appends, and `2>` / `&>` are stream redirections we do
                // not try to reason about beyond the target.
                if segment[index + 1..].starts_with('>') {
                    return None;
                }
                let tail = segment[index + 1..].trim_start();
                let target = tail.split_whitespace().next()?;
                if target.is_empty() || target.starts_with('&') {
                    return None;
                }
                let path = expand(target.trim_matches(['"', '\'']));
                let non_empty = std::fs::metadata(&path)
                    .map(|meta| meta.is_file() && meta.len() > 0)
                    .unwrap_or(false);
                return non_empty.then_some(path);
            }
            _ => {}
        }
    }
    None
}

fn expand(value: &str) -> PathBuf {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    if value == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        super::workspace::effective_workdir().join(path)
    }
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Splits on the operators that start a new command, respecting quotes.
/// A guard that missed `safe && rm -rf x` would be no guard at all.
fn split_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' if quote != Some('\'') => {
                current.push(character);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' | '"' if quote == Some(character) => {
                quote = None;
                current.push(character);
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(character);
                current.push(character);
            }
            '&' | '|' | ';' | '\n' if quote.is_none() => {
                // Swallow a doubled operator so `&&` does not leave a stray `&`.
                if (character == '&' || character == '|') && chars.peek() == Some(&character) {
                    chars.next();
                }
                segments.push(std::mem::take(&mut current));
            }
            _ => current.push(character),
        }
    }
    segments.push(current);
    segments
        .into_iter()
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Splits a segment into words, dropping the quotes that grouped them.
fn tokenize(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = segment.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' if quote != Some('\'') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            '\'' | '"' if quote == Some(character) => {
                quote = None;
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(character);
                started = true;
            }
            character if character.is_whitespace() && quote.is_none() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            // A redirect target is not an operand; `truncating_redirect`
            // handles it separately.
            '>' | '<' if quote.is_none() => break,
            _ => {
                current.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

tokio::task_local! {
    /// Set for the whole of a subagent's tool loop.
    static SUBAGENT: ();
}

/// Marks a future as running inside a subagent.
pub async fn in_subagent<F: std::future::Future>(future: F) -> F::Output {
    SUBAGENT.scope((), future).await
}

fn is_subagent() -> bool {
    SUBAGENT.try_with(|_| ()).is_ok()
}

/// The environment override, which wins over the config so a one-off run can
/// turn the guard off without editing anything.
fn disabled_by_environment() -> bool {
    std::env::var_os("MIYU_NO_DELETE_GUARD").is_some_and(|value| value != "0")
}

/// Runs a shell command past the guard. Returns the command to actually run,
/// or an error carrying the reason it was stopped.
///
/// A `None` command means the user chose the Trash instead and the deletion has
/// already been carried out recoverably — the caller must not run anything.
pub async fn screen_command(
    command: &str,
    enabled: bool,
    progress: &crate::tools::ToolProgress,
) -> anyhow::Result<Option<()>> {
    let verdict = assess_command(command);
    // Fuses are not a preference: `rm -rf /` is refused even with the guard off.
    if let Verdict::Refuse(reason) = &verdict {
        anyhow::bail!("{}", refusal(reason));
    }
    if !enabled || disabled_by_environment() {
        return Ok(Some(()));
    }
    let Verdict::Ask(preview) = verdict else {
        return Ok(Some(()));
    };
    // A subagent runs unattended by design: it has no conversation with the
    // user, so a confirmation there would be a dialog nobody asked for about a
    // decision nobody can weigh. Deleting is not its job.
    if is_subagent() {
        anyhow::bail!("{}", subagent_refusal());
    }
    match confirm(&preview, progress).await {
        Decision::Proceed => Ok(Some(())),
        Decision::DivertToTrash => {
            divert_to_trash(&preview.targets)?;
            Ok(None)
        }
        Decision::Deny => anyhow::bail!("{}", denial()),
    }
}

/// Same judgement for a set of paths a tool resolved itself.
pub async fn screen_paths(
    targets: Vec<std::path::PathBuf>,
    enabled: bool,
    progress: &crate::tools::ToolProgress,
) -> anyhow::Result<()> {
    let verdict = assess_paths(targets);
    if let Verdict::Refuse(reason) = &verdict {
        anyhow::bail!("{}", refusal(reason));
    }
    if !enabled || disabled_by_environment() {
        return Ok(());
    }
    let Verdict::Ask(preview) = verdict else {
        return Ok(());
    };
    if is_subagent() {
        anyhow::bail!("{}", subagent_refusal());
    }
    match confirm(&preview, progress).await {
        // Diverting is offered for shell deletions, where we would otherwise
        // run the command; a tool that unlinks its own resolved paths gets the
        // plain allow/deny pair, so treat anything but a yes as a no.
        Decision::Proceed => Ok(()),
        Decision::DivertToTrash | Decision::Deny => anyhow::bail!("{}", denial()),
    }
}

fn refusal(reason: &str) -> String {
    format!(
        "{} {reason}. {}",
        crate::i18n::text("refused:", "已拒绝："),
        crate::i18n::text(
            "Narrow the target and try again.",
            "请把目标缩小到确切的路径后重试。"
        )
    )
}

fn subagent_refusal() -> String {
    crate::i18n::text(
        "a subagent must not delete files; report what should be removed and let the main agent handle it, or use trash_path",
        "子代理不能删除文件；请把要删除的内容回报给主代理处理，或改用 trash_path",
    )
    .to_string()
}

fn denial() -> String {
    crate::i18n::text(
        "the user declined this deletion; do not retry it and do not look for another way to delete the same thing",
        "用户拒绝了这次删除；不要重试，也不要换一种写法删同样的东西",
    )
    .to_string()
}

fn divert_to_trash(targets: &[std::path::PathBuf]) -> anyhow::Result<()> {
    for target in targets {
        trash::delete(target).map_err(|error| {
            anyhow::anyhow!("could not move {} to Trash: {error}", target.display())
        })?;
    }
    Ok(())
}

/// What the user decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Proceed,
    /// Do not run the deletion; move the targets to the Trash instead.
    DivertToTrash,
    Deny,
}

// Compared by value against the answer, so they are constants rather than
// strings rebuilt at two call sites.
fn label_deny() -> &'static str {
    crate::i18n::text("Deny", "拒绝")
}
fn label_trash() -> &'static str {
    crate::i18n::text("Move to Trash instead", "改用回收站")
}
fn label_proceed() -> &'static str {
    crate::i18n::text("Allow once", "允许一次")
}

/// Asks the user, and treats anything other than a clear yes as a no.
pub async fn confirm(preview: &Preview, progress: &crate::tools::ToolProgress) -> Decision {
    use crate::question::QuestionResponse;

    let response = progress.request_approval(prompt_for(preview)).await;
    let Some(answer) = (match &response {
        QuestionResponse::Answered(answers) => answers.first().and_then(|first| first.first()),
        // Closed, cancelled, or nobody home. An irreversible delete is not
        // something to do on a shrug.
        _ => None,
    }) else {
        return Decision::Deny;
    };

    if answer == label_proceed() {
        Decision::Proceed
    } else if answer == label_trash() && preview.can_divert_to_trash() {
        Decision::DivertToTrash
    } else {
        Decision::Deny
    }
}

/// Builds the question. The first line says what is about to happen, the
/// second says what it costs — and the second line is ours, never the model's.
fn prompt_for(preview: &Preview) -> crate::question::QuestionRequest {
    use crate::question::{QuestionOption, QuestionPrompt, QuestionRequest};

    let mut options = vec![QuestionOption {
        label: label_deny().to_string(),
        description: crate::i18n::text("Nothing is deleted.", "什么都不会被删除。").to_string(),
    }];
    if preview.can_divert_to_trash() {
        options.push(QuestionOption {
            label: label_trash().to_string(),
            description: crate::i18n::text(
                "Delete recoverably — you can still get these back.",
                "改成可恢复的删除，之后还能找回来。",
            )
            .to_string(),
        });
    }
    options.push(QuestionOption {
        label: label_proceed().to_string(),
        description: crate::i18n::text("Delete permanently, as asked.", "按原样永久删除。")
            .to_string(),
    });

    QuestionRequest {
        questions: vec![QuestionPrompt {
            header: crate::i18n::text("Confirmation needed", "需要确认").to_string(),
            question: body_for(preview),
            options,
            multiple: false,
            // The answer is a decision, not free text; a typed reply here has
            // no meaning and would only be another thing to misread.
            custom: false,
        }],
    }
}

fn body_for(preview: &Preview) -> String {
    if !preview.understood {
        let command = match &preview.source {
            Source::Shell(command) => command.as_str(),
            Source::ApplyPatch => "",
        };
        return format!(
            "{} {} — {}",
            crate::i18n::text(
                "About to run a command that may delete files.",
                "准备运行一条可能会删除文件的命令。"
            ),
            crate::i18n::text(
                "I could not work out what it would delete, so I cannot tell you the impact.",
                "我没看懂它具体会删什么，无法告诉你影响范围。"
            ),
            clip(&one_line(command), 300),
        );
    }

    let where_ = preview
        .targets
        .iter()
        .map(|target| display_path(target))
        .collect::<Vec<_>>()
        .join("，");
    // "21 files" must mean files; the directories holding them are structure.
    // When a target holds nothing but directories, fall back to entries so the
    // sentence does not claim zero.
    let (number, noun_zh, noun_en) = if preview.files > 0 {
        (preview.files, "个文件", "file(s)")
    } else {
        (preview.entries, "个项目", "item(s)")
    };
    let number = if preview.truncated {
        format!("{number}+")
    } else {
        number.to_string()
    };
    let count = if crate::i18n::is_zh() {
        format!("{number} {noun_zh}")
    } else {
        format!("{number} {noun_en}")
    };
    let size = readable_bytes(preview.bytes);
    let where_ = clip(&one_line(&where_), 200);
    // Leads with what is about to be lost, because that is what the person is
    // deciding about. Paths are shortened to their tail (see display_path) so
    // the sentence still fits when the panel clips it to the terminal width.
    let single_file = preview.targets.len() == 1 && preview.entries == 1;
    if crate::i18n::is_zh() {
        if single_file {
            format!("{where_}（{size}）将被彻底删除，不进回收站")
        } else {
            format!("{where_} 里的 {count}（{size}）将被彻底删除，不进回收站")
        }
    } else if single_file {
        format!("{where_} ({size}) will be permanently deleted, not moved to the Trash")
    } else {
        format!(
            "{count} ({size}) under {where_} will be permanently deleted, not moved to the Trash"
        )
    }
}

/// Shortens a path to the part that identifies it. The tail is what tells a
/// person which folder this is; a deep prefix only costs width the panel does
/// not have.
fn display_path(path: &Path) -> String {
    let full = if let Some(home) = home_dir() {
        match path.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    } else {
        path.display().to_string()
    };
    if full.chars().count() <= 44 {
        return full;
    }
    let parts: Vec<&str> = full.split('/').filter(|part| !part.is_empty()).collect();
    let tail = parts
        .iter()
        .rev()
        .take(3)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    format!("…/{tail}")
}

fn readable_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// The question field rejects control characters outright, so a multi-line
/// command or path list has to be flattened before it goes in.
fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The question field has a hard character limit; overflowing it would make
/// `validate()` reject the prompt and turn a confirmation into a tool error.
fn clip(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let head: String = value.chars().take(limit).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that is *not* under the temp roots the guard
    /// exempts — otherwise every fixture would be silently allowed.
    fn workdir() -> tempfile::TempDir {
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/target/delete-guard-tests");
        std::fs::create_dir_all(base).unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    fn ask(command: &str) -> Preview {
        match assess_command(command) {
            Verdict::Ask(preview) => preview,
            other => panic!("expected Ask for {command:?}, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_commands_are_not_mistaken_for_deletions() {
        // The old substring guard rejected this because of the bare `>`.
        assert_eq!(assess_command(r#"grep -rn "a->b" src/"#), Verdict::Allow);
        assert_eq!(assess_command("cargo build"), Verdict::Allow);
        assert_eq!(assess_command("git status"), Verdict::Allow);
        assert_eq!(assess_command("ls -la /etc"), Verdict::Allow);
        // "remove" as prose, not as a command.
        assert_eq!(
            assess_command(r#"echo "remove the old files""#),
            Verdict::Allow
        );
    }

    #[test]
    fn a_deletion_hidden_behind_a_safe_command_is_still_found() {
        let temp = workdir();
        let victim = temp.path().join("victim.txt");
        std::fs::write(&victim, "data").unwrap();
        let preview = ask(&format!("git status && rm -rf {}", victim.display()));
        assert_eq!(preview.targets, vec![victim]);
        assert!(preview.understood);
    }

    #[test]
    fn every_separator_starts_a_new_command() {
        let temp = workdir();
        let victim = temp.path().join("v");
        std::fs::write(&victim, "x").unwrap();
        for separator in ["&&", "||", ";", "|", "\n", "&"] {
            let command = format!("echo hi {separator} rm {}", victim.display());
            assert!(
                matches!(assess_command(&command), Verdict::Ask(_)),
                "separator {separator:?} let a deletion through"
            );
        }
    }

    #[test]
    fn an_empty_variable_that_would_become_the_root_is_refused() {
        for command in [r#"rm -rf "$BUILD_DIR"/"#, "rm -rf ${OUT}", "rm -rf $TARGET"] {
            assert!(
                matches!(assess_command(command), Verdict::Refuse(_)),
                "{command:?} should be refused outright"
            );
        }
        // A variable with a real path after it is a normal, answerable request.
        assert!(!matches!(
            assess_command("rm -rf $HOME/projects/build"),
            Verdict::Refuse(_)
        ));
    }

    #[test]
    fn deleting_the_filesystem_root_or_home_is_refused() {
        assert!(matches!(assess_command("rm -rf /"), Verdict::Refuse(_)));
        assert!(matches!(assess_command("rm -rf ~"), Verdict::Refuse(_)));
    }

    #[test]
    fn scratch_directories_are_not_worth_interrupting_for() {
        assert_eq!(assess_command("rm -rf /tmp/miyu-scratch"), Verdict::Allow);
        assert_eq!(assess_command("rm /var/tmp/build.log"), Verdict::Allow);
        // But the scratch root itself is not a target we silently accept.
        assert!(matches!(assess_command("rm -rf /tmp"), Verdict::Ask(_)));
    }

    #[test]
    fn redirects_only_count_when_they_would_overwrite_something() {
        let temp = workdir();
        let fresh = temp.path().join("new.log");
        let existing = temp.path().join("existing.txt");
        std::fs::write(&existing, "hundreds of lines").unwrap();

        assert_eq!(
            assess_command(&format!("cargo build > {}", fresh.display())),
            Verdict::Allow
        );
        assert_eq!(
            assess_command(&format!("echo x >> {}", existing.display())),
            Verdict::Allow,
            "appending does not destroy"
        );
        let preview = ask(&format!("echo x > {}", existing.display()));
        assert_eq!(preview.targets, vec![existing]);
    }

    #[test]
    fn find_delete_is_recognised_but_plain_find_is_not() {
        let temp = workdir();
        std::fs::write(temp.path().join("a.o"), "x").unwrap();
        let root = temp.path().display().to_string();
        assert_eq!(
            assess_command(&format!("find {root} -name '*.o'")),
            Verdict::Allow
        );
        let preview = ask(&format!("find {root} -name '*.o' -delete"));
        assert_eq!(preview.targets, vec![temp.path().to_path_buf()]);
        assert!(matches!(
            assess_command(&format!("find {root} -type f -exec rm {{}} ;")),
            Verdict::Ask(_)
        ));
    }

    #[test]
    fn truncate_to_zero_destroys_but_other_sizes_do_not() {
        let temp = workdir();
        let file = temp.path().join("f");
        std::fs::write(&file, "data").unwrap();
        assert!(matches!(
            assess_command(&format!("truncate -s 0 {}", file.display())),
            Verdict::Ask(_)
        ));
        assert_eq!(
            assess_command(&format!("truncate -s 10M {}", file.display())),
            Verdict::Allow
        );
    }

    #[test]
    fn an_unreadable_deletion_asks_without_offering_the_trash() {
        // `xargs rm` is a deletion we do not resolve targets for.
        let preview = ask("cat list.txt | xargs rm");
        assert!(!preview.understood);
        assert!(!preview.can_divert_to_trash());
        assert!(preview.targets.is_empty());
    }

    #[test]
    fn the_preview_counts_what_is_really_there() {
        let temp = workdir();
        let nested = temp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("one"), "12345").unwrap();
        std::fs::write(temp.path().join("two"), "123").unwrap();

        let preview = ask(&format!("rm -rf {}", temp.path().display()));
        // root + a + b + one + two
        assert_eq!(preview.entries, 5);
        // Only the two real files: "2 files" must not silently include the
        // directories that hold them.
        assert_eq!(preview.files, 2);
        assert_eq!(preview.bytes, 8);
        assert!(!preview.truncated);
        assert!(preview.can_divert_to_trash());
    }

    #[test]
    fn apply_patch_deletions_go_through_the_same_judgement() {
        let temp = workdir();
        let file = temp.path().join("gone.rs");
        std::fs::write(&file, "fn main() {}").unwrap();
        let Verdict::Ask(preview) = assess_paths(vec![file.clone()]) else {
            panic!("expected Ask");
        };
        assert_eq!(preview.source, Source::ApplyPatch);
        assert_eq!(preview.entries, 1);
        assert_eq!(preview.files, 1);
        assert!(matches!(
            assess_paths(vec![PathBuf::from("/")]),
            Verdict::Refuse(_)
        ));
    }

    #[test]
    fn the_prompt_survives_validation_even_with_absurd_input() {
        // A prompt that fails validate() becomes a tool error instead of a
        // confirmation, which would silently turn the guard off.
        let long = Preview {
            targets: (0..200)
                .map(|index| PathBuf::from(format!("/very/long/path/number/{index}/file.txt")))
                .collect(),
            files: 0,
            entries: 4321,
            truncated: true,
            bytes: 99_999_999_999,
            understood: true,
            source: Source::Shell("x".repeat(5000)),
        };
        prompt_for(&long).validate().expect("long target list");

        let unreadable = Preview {
            targets: Vec::new(),
            files: 0,
            entries: 0,
            truncated: false,
            bytes: 0,
            understood: false,
            source: Source::Shell("y".repeat(5000)),
        };
        prompt_for(&unreadable)
            .validate()
            .expect("unreadable command");
    }

    #[test]
    fn the_trash_option_appears_only_when_targets_are_known() {
        let known = Preview {
            targets: vec![PathBuf::from("/a/b")],
            files: 0,
            entries: 1,
            truncated: false,
            bytes: 4,
            understood: true,
            source: Source::ApplyPatch,
        };
        let labels: Vec<String> = prompt_for(&known).questions[0]
            .options
            .iter()
            .map(|option| option.label.clone())
            .collect();
        assert!(labels.contains(&label_trash().to_string()));
        // Refusing is first so that the default cursor lands on the safe answer.
        assert_eq!(labels.first().unwrap(), label_deny());

        let unknown = Preview {
            understood: false,
            targets: Vec::new(),
            ..known
        };
        let labels: Vec<String> = prompt_for(&unknown).questions[0]
            .options
            .iter()
            .map(|option| option.label.clone())
            .collect();
        assert!(
            !labels.contains(&label_trash().to_string()),
            "cannot offer the Trash when we do not know what to move"
        );
    }

    #[test]
    fn the_sentence_reads_like_a_sentence() {
        let temp = workdir();
        let dir = temp.path().join("素材");
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..3 {
            std::fs::write(dir.join(format!("clip-{index}.mov")), "abc").unwrap();
        }
        let body = body_for(&ask(&format!("rm -rf {}", dir.display())));
        // Assert on the count phrase, not on a bare digit: the temp directory
        // in the path carries a random name that regularly contains one, which
        // made this test fail about one run in eight.
        assert!(body.contains("3 个文件"), "{body}");
        assert!(
            !body.contains("4 个文件"),
            "the directory itself must not be counted as a file: {body}"
        );
        assert!(
            body.contains("不进回收站") || body.contains("not moved to the Trash"),
            "{body}"
        );
        assert!(!body.chars().any(char::is_control));

        // A lone file should not be described as "N files inside it".
        let single = temp.path().join("one.txt");
        std::fs::write(&single, "xy").unwrap();
        let body = body_for(&ask(&format!("rm {}", single.display())));
        assert!(
            !body.contains("里的"),
            "a single file has no inside: {body}"
        );
    }

    #[test]
    fn the_prompt_takes_a_decision_not_free_text() {
        let preview = Preview {
            targets: vec![PathBuf::from("/a")],
            files: 0,
            entries: 1,
            truncated: false,
            bytes: 0,
            understood: true,
            source: Source::ApplyPatch,
        };
        let prompt = &prompt_for(&preview).questions[0];
        assert!(!prompt.custom);
        assert!(!prompt.multiple);
    }

    #[tokio::test]
    async fn a_subagent_is_told_to_hand_the_deletion_back() {
        let temp = workdir();
        let victim = temp.path().join("gone");
        std::fs::create_dir_all(&victim).unwrap();
        let command = format!("rm -rf {}", victim.display());
        let error = in_subagent(screen_command(
            &command,
            true,
            &crate::tools::ToolProgress::default(),
        ))
        .await
        .expect_err("a subagent must not delete");
        assert!(
            error.to_string().contains("trash_path"),
            "the error should point at what to do instead: {error}"
        );
        assert!(victim.exists());
    }

    #[tokio::test]
    async fn outside_a_subagent_the_same_command_reaches_the_user() {
        let temp = workdir();
        let victim = temp.path().join("gone");
        std::fs::create_dir_all(&victim).unwrap();
        let command = format!("rm -rf {}", victim.display());
        // No frontend, so it still fails — but as a declined confirmation,
        // not as "subagents may not delete".
        let error = screen_command(&command, true, &crate::tools::ToolProgress::default())
            .await
            .expect_err("no frontend means no approval");
        assert!(!error.to_string().contains("trash_path"));
    }

    #[tokio::test]
    async fn with_nobody_to_ask_the_deletion_is_refused() {
        let preview = Preview {
            targets: vec![PathBuf::from("/a")],
            files: 0,
            entries: 1,
            truncated: false,
            bytes: 0,
            understood: true,
            source: Source::ApplyPatch,
        };
        // Default ToolProgress has no sender: a background job or a platform
        // turn. Failing closed is the whole point.
        let decision = confirm(&preview, &crate::tools::ToolProgress::default()).await;
        assert_eq!(decision, Decision::Deny);
    }

    #[test]
    fn quotes_hold_a_path_together() {
        let temp = workdir();
        let spaced = temp.path().join("two words.txt");
        std::fs::write(&spaced, "x").unwrap();
        let preview = ask(&format!(r#"rm "{}""#, spaced.display()));
        assert_eq!(preview.targets, vec![spaced]);
    }
}
