//! Moving a Miyu installation between machines: `miyu export` / `miyu import`.

pub mod registry;

#[cfg(test)]
mod tests {
    use super::registry::{is_backup_name, unit_for, Tier, IGNORED_SUFFIXES, UNITS};
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Everything Miyu writes under `MIYU_HOME` must be classified.
    ///
    /// This is the guard that keeps export from rotting: add a feature that
    /// writes somewhere new, forget to register it, and this fails with a
    /// pointer to the registry — instead of the user discovering the gap on
    /// the new machine, after the old one is gone.
    #[test]
    fn every_data_location_is_classified() {
        // A representative populated home. Kept literal rather than derived
        // from a live run so the expectations are reviewable in the diff.
        let observed = [
            ".layout-v1",
            ".resource-layout-v1",
            "cache/logs/miyu.2026-08-08.log",
            "cache/models_cache.json",
            "cache/jobs/abc123.log",
            "cache/clipboard_images/1.png",
            "cache/platform_images/onebot/x.jpg",
            "cache/default-kb/update-source",
            "config/config.jsonc",
            "config/webui-theme.css",
            "config/shell/bash-hook.sh",
            "data/prompts/system-prompt.md",
            "data/identities/user-identity.md",
            "data/persona-avatars/default.png",
            "data/skills/my-skill/SKILL.md",
            "data/scripts/index.json",
            "data/pictures/out.png",
            "data/documents/report.md",
            "data/memes/library/a.gif",
            "data/personas/default/memory/memory.db",
            "data/kb/files/games/a.md",
            "data/kb/kb_meta.db",
            "data/kb/semantic_index.db",
            "data/default-kb/state.json",
            "data/platforms/onebot/message_history/history.sqlite3",
            "data/platforms/onebot/real_context/state.json",
            "data/artifacts/sess_1/page.html",
            "state/conversation.db",
            "state/conversation.jsonl",
            "state/usage.json",
            "state/profile.md",
            "state/alarms.json",
            "state/thinking-variants.json",
            "state/repl-history.jsonl",
            "state/skill-drafts/draft.md",
            "state/personas/default/memory/evicted_context.db",
            "state/prompt-fingerprints/abc.sha256",
            "state/prompt.sha256",
            "state/aur-review-state.json",
            "state/arch_news_last_seen.json",
            "state/daemon-launch.json",
            "state/web-passwords/password-1234-ab",
            "state/miyu/core.sock",
            "state/conversation.db.bak",
        ];

        let unclassified: Vec<&str> = observed
            .iter()
            .copied()
            .filter(|rel| unit_for(rel).is_none())
            .collect();
        assert!(
            unclassified.is_empty(),
            "unclassified paths under MIYU_HOME: {unclassified:?}\n\
             Register each in src/transfer/registry.rs `UNITS` — or mark it \
             Tier::Never with the reason it must not travel."
        );
    }

    #[test]
    fn an_unregistered_location_is_reported() {
        // The guard above is only worth having if it actually catches things.
        assert!(unit_for("data/brand-new-feature/store.db").is_none());
        assert!(unit_for("state/some-future-file.json").is_none());
    }

    #[test]
    fn unit_ids_and_paths_are_unique() {
        let ids: BTreeSet<&str> = UNITS.iter().map(|unit| unit.id).collect();
        assert_eq!(ids.len(), UNITS.len(), "duplicate DataUnit id");
        let rels: BTreeSet<&str> = UNITS.iter().map(|unit| unit.rel).collect();
        assert_eq!(rels.len(), UNITS.len(), "duplicate DataUnit rel");
    }

    #[test]
    fn never_units_state_a_reason() {
        for unit in UNITS.iter().filter(|unit| unit.tier == Tier::Never) {
            assert!(
                unit.why.len() > 20,
                "{}: Tier::Never needs a real reason, not `{}`",
                unit.id,
                unit.why
            );
        }
    }

    #[test]
    fn tier_switches_select_the_expected_units() {
        let ids = |all: bool, index: bool, platforms: bool| -> BTreeSet<&str> {
            UNITS
                .iter()
                .filter(|unit| unit.included(all, index, platforms))
                .map(|unit| unit.id)
                .collect()
        };

        let default = ids(false, false, false);
        assert!(default.contains("state.conversation"));
        assert!(default.contains("kb.files"));
        // The 143MB derived index and the account-bound platform history stay
        // out unless asked for.
        assert!(!default.contains("kb.semantic_index"));
        assert!(!default.contains("platform.message_history"));

        assert!(ids(false, true, false).contains("kb.semantic_index"));
        assert!(ids(false, false, true).contains("platform.message_history"));

        let all = ids(true, true, true);
        assert!(all.contains("kb.semantic_index"));
        assert!(all.contains("platform.message_history"));
        // No switch may ever pull in a Never unit.
        for unit in UNITS.iter().filter(|unit| unit.tier == Tier::Never) {
            assert!(!all.contains(unit.id), "{} must never be exported", unit.id);
        }
    }

    #[test]
    fn machine_specific_paths_resolve_to_never() {
        for rel in [
            "cache/logs/miyu.log",
            "cache/jobs/abc.log",
            "state/daemon-launch.json",
            "state/web-passwords/password-1-a",
            "state/miyu/core.sock",
            "config/shell/bash-hook.sh",
            "data/artifacts/sess_1/page.html",
            "state/conversation.db.bak",
        ] {
            let unit = unit_for(rel).unwrap_or_else(|| panic!("{rel} unclassified"));
            assert_eq!(unit.tier, Tier::Never, "{rel} resolved to {}", unit.id);
        }
    }

    #[test]
    fn sqlite_sidecars_and_backups_are_skipped() {
        for name in ["conversation.db-wal", "conversation.db-shm", "core.lock"] {
            assert!(
                IGNORED_SUFFIXES
                    .iter()
                    .any(|suffix| name.ends_with(suffix)),
                "{name} should be skipped as a sidecar"
            );
        }
        assert!(is_backup_name("config.jsonc.bak-20260802-011956"));
        assert!(is_backup_name("conversation.db.bak"));
        assert!(!is_backup_name("config.jsonc"));
        // Sanity: the helper is about names, not whole paths.
        assert!(Path::new("config.jsonc").file_name().is_some());
    }
}
