use crate::{InvocationResult, OutputFormat, SkillsCommand, invalid_invocation};

struct Skill {
    name: &'static str,
    title: &'static str,
    body: &'static str,
}

const SKILLS: &[Skill] = &[
    Skill {
        name: "core",
        title: "Core agent workflow",
        body: include_str!("skills/core.md"),
    },
    Skill {
        name: "library",
        title: "Find, inspect, and decide",
        body: include_str!("skills/library.md"),
    },
    Skill {
        name: "workspace",
        title: "Work in a Photo Workspace",
        body: include_str!("skills/workspace.md"),
    },
    Skill {
        name: "darktable",
        title: "Edit, preview, and export with darktable",
        body: include_str!("skills/darktable.md"),
    },
    Skill {
        name: "recovery",
        title: "Trash, removal, Restore, and Location Recovery",
        body: include_str!("skills/recovery.md"),
    },
];

pub(crate) fn invoke(command: Option<&SkillsCommand>) -> InvocationResult {
    match command {
        None | Some(SkillsCommand::List) => {
            let mut stdout = String::new();
            for skill in SKILLS {
                stdout.push_str(skill.name);
                stdout.push_str(" — ");
                stdout.push_str(skill.title);
                stdout.push('\n');
            }
            success(stdout)
        }
        Some(SkillsCommand::Get { name }) => match SKILLS.iter().find(|skill| skill.name == name) {
            Some(skill) => success(skill.body.to_owned()),
            None => invalid_invocation(
                OutputFormat::Text,
                format!("Unknown skill {name:?}. Run `slipstream skills list`."),
            ),
        },
    }
}

fn success(stdout: String) -> InvocationResult {
    InvocationResult {
        exit_code: 0,
        stdout,
        committed_preview_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command};
    use clap::Parser;

    #[test]
    fn lists_the_bundled_catalog_in_order() {
        let result = invoke(None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.stdout,
            "core — Core agent workflow\n\
             library — Find, inspect, and decide\n\
             workspace — Work in a Photo Workspace\n\
             darktable — Edit, preview, and export with darktable\n\
             recovery — Trash, removal, Restore, and Location Recovery\n"
        );
    }

    #[test]
    fn returns_exact_markdown_without_an_envelope() {
        let result = invoke(Some(&SkillsCommand::Get {
            name: "core".to_owned(),
        }));
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.starts_with("# Core agent workflow\n"));
        assert!(!result.stdout.contains("\"data\""));
    }

    #[test]
    fn rejects_an_unknown_skill() {
        let result = invoke(Some(&SkillsCommand::Get {
            name: "missing".to_owned(),
        }));
        assert_eq!(result.exit_code, 2);
        assert!(result.stdout.contains("invalid_input"));
    }

    #[test]
    fn parses_bare_skills_and_get() {
        assert!(matches!(
            Cli::try_parse_from(["slipstream", "skills"])
                .unwrap()
                .command,
            Command::Skills { command: None }
        ));
        assert!(matches!(
            Cli::try_parse_from(["slipstream", "skills", "get", "core"])
                .unwrap()
                .command,
            Command::Skills {
                command: Some(SkillsCommand::Get { name })
            } if name == "core"
        ));
    }
}
