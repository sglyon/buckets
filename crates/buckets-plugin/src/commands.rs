//! Static command-line metadata. Discovery happens off the input path; command
//! arguments stay argv values and are never reparsed as shell syntax.
use crate::CommandDescriptor;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_COMMAND_ARGUMENTS: usize = 256;
pub const MAX_COMMAND_ARGUMENT_BYTES: usize = 32_768;
pub const MAX_COMMAND_ARGUMENTS_BYTES: usize = 131_072;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandLocation {
    Local,
    Ssh,
    S3,
    Git,
    Plugin,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandArgumentKind {
    #[default]
    Text,
    Path,
    Directory,
    Choice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandArgument {
    pub name: String,
    #[serde(default)]
    pub kind: CommandArgumentKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub variadic: bool,
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandLineSpec {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub arguments: Vec<CommandArgument>,
    /// Empty accepts every provider. These are static hints and dispatch checks,
    /// not probes for host/network availability.
    #[serde(default)]
    pub locations: Vec<CommandLocation>,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandContext {
    pub location: String,
    #[serde(default)]
    pub selected: Vec<String>,
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}
fn argument(value: &str) -> bool {
    value.len() <= MAX_COMMAND_ARGUMENT_BYTES && !value.contains('\0')
}

impl CommandDescriptor {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 128
                && self
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)),
            "Invalid plugin command id"
        );
        ensure!(
            !self.title.is_empty()
                && self.title.chars().count() <= 128
                && !self.title.chars().any(char::is_control),
            "Invalid plugin command title"
        );
        ensure!(
            self.description.chars().count() <= 512
                && !self
                    .description
                    .chars()
                    .any(|c| c.is_control() && c != '\n'),
            "Invalid plugin command description"
        );
        if let Some(shortcut) = &self.shortcut {
            ensure!(
                !shortcut.is_empty()
                    && shortcut.len() <= 64
                    && shortcut
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "- +".contains(c)),
                "Invalid plugin shortcut"
            );
        }
        if let Some(spec) = &self.command_line {
            spec.validate()?;
        }
        Ok(())
    }
}

impl CommandLineSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(token(&self.name), "Invalid command-line name");
        ensure!(self.aliases.len() <= 16, "Command has more than 16 aliases");
        let mut names = BTreeSet::from([self.name.as_str()]);
        for alias in &self.aliases {
            ensure!(
                token(alias) && names.insert(alias),
                "Invalid or duplicate command-line alias"
            );
        }
        ensure!(
            self.arguments.len() <= 32,
            "Command has more than 32 argument definitions"
        );
        let mut names = BTreeSet::new();
        let mut optional = false;
        for (i, arg) in self.arguments.iter().enumerate() {
            ensure!(
                token(&arg.name) && names.insert(&arg.name),
                "Invalid or duplicate command argument name"
            );
            ensure!(
                !arg.variadic || i + 1 == self.arguments.len(),
                "Only the final command argument may be variadic"
            );
            ensure!(
                !optional || !arg.required,
                "Required command arguments must precede optional arguments"
            );
            optional |= !arg.required;
            ensure!(
                arg.choices.len() <= 128,
                "Command argument has more than 128 choices"
            );
            ensure!(
                (arg.kind == CommandArgumentKind::Choice) != arg.choices.is_empty(),
                "Only choice arguments require nonempty choices"
            );
            let mut choices = BTreeSet::new();
            ensure!(
                arg.choices.iter().all(|choice| !choice.is_empty()
                    && choice.chars().count() <= 256
                    && !choice.chars().any(char::is_control)
                    && choices.insert(choice)),
                "Invalid or duplicate command argument choice"
            );
        }
        let mut locations = BTreeSet::new();
        ensure!(
            self.locations
                .iter()
                .all(|location| locations.insert(*location as u8)),
            "Duplicate command location restriction"
        );
        if let Some(reason) = &self.unavailable_reason {
            ensure!(
                !reason.is_empty()
                    && reason.chars().count() <= 512
                    && !reason.chars().any(char::is_control),
                "Invalid command availability reason"
            );
        }
        Ok(())
    }

    pub fn check_location(&self, location: &str) -> Result<()> {
        let scheme = if location.starts_with('/') || location.starts_with("file://") {
            Some(CommandLocation::Local)
        } else if location.starts_with("ssh://") {
            Some(CommandLocation::Ssh)
        } else if location.starts_with("s3://") {
            Some(CommandLocation::S3)
        } else if location.starts_with("git://") {
            Some(CommandLocation::Git)
        } else if location.starts_with("plugin://") {
            Some(CommandLocation::Plugin)
        } else {
            None
        };
        ensure!(
            self.locations.is_empty()
                || scheme.is_some_and(|scheme| self.locations.contains(&scheme)),
            "{}",
            self.unavailable_reason
                .as_deref()
                .unwrap_or("Command is unavailable for this location provider")
        );
        Ok(())
    }

    pub fn validate_arguments(&self, argv: &[String]) -> Result<()> {
        self.validate()?;
        ensure!(
            argv.len() <= MAX_COMMAND_ARGUMENTS
                && argv.iter().all(|arg| argument(arg))
                && argv.iter().map(String::len).sum::<usize>() <= MAX_COMMAND_ARGUMENTS_BYTES,
            "Command arguments exceed limits or contain NUL"
        );
        let required = self.arguments.iter().filter(|arg| arg.required).count();
        let variadic = self.arguments.last().is_some_and(|arg| arg.variadic);
        ensure!(argv.len() >= required, "Missing required command argument");
        ensure!(
            variadic || argv.len() <= self.arguments.len(),
            "Too many command arguments"
        );
        for (index, value) in argv.iter().enumerate() {
            let definition = self
                .arguments
                .get(index)
                .or_else(|| self.arguments.last())
                .expect("argument count validated");
            if definition.kind == CommandArgumentKind::Choice {
                ensure!(
                    definition.choices.contains(value),
                    "Invalid value for argument {}",
                    definition.name
                );
            }
        }
        Ok(())
    }
}

impl CommandContext {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.location.is_empty() && argument(&self.location),
            "Invalid command location"
        );
        ensure!(
            self.selected.len() <= 10_000
                && self
                    .selected
                    .iter()
                    .all(|value| !value.is_empty() && argument(value))
                && self.selected.iter().map(String::len).sum::<usize>() <= 524_288,
            "Command selection exceeds limits or contains NUL"
        );
        Ok(())
    }
}
