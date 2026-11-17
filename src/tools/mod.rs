pub mod clipboard_image;
pub mod text;

use crate::clipboard::ClipboardSummary;
use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionCategory {
    Image,
    Text,
    Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputKind {
    Image,
    FileDrop,
    VirtualFile,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputKind {
    Image,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SideEffect {
    ClipboardRead,
    ClipboardWrite,
    ReferencedFileRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ToolDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: ActionCategory,
    pub accepted_inputs: &'static [InputKind],
    pub output: OutputKind,
    pub side_effects: &'static [SideEffect],
    pub supports_preview: bool,
    pub supports_cancel: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Unavailable { reason: &'static str },
}

impl Availability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

impl ClipboardSummary {
    pub fn supports(&self, kind: InputKind) -> bool {
        match kind {
            InputKind::Image => self.image,
            InputKind::FileDrop => self.file_drop,
            InputKind::VirtualFile => self.virtual_file,
            InputKind::Text => self.unicode_text,
        }
    }
}

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn run(&self) -> Result<String>;

    fn id(&self) -> &'static str {
        self.descriptor().id
    }
    fn name(&self) -> &'static str {
        self.descriptor().name
    }
    fn description(&self) -> &'static str {
        self.descriptor().description
    }
    fn availability(&self, clipboard: &ClipboardSummary) -> Availability {
        let descriptor = self.descriptor();
        if descriptor
            .accepted_inputs
            .iter()
            .any(|kind| clipboard.supports(*kind))
        {
            Availability::Available
        } else if descriptor.accepted_inputs.contains(&InputKind::Text) {
            Availability::Unavailable {
                reason: "剪贴板中没有 Unicode 文本（未更改原内容）",
            }
        } else {
            Availability::Unavailable {
                reason: "剪贴板中没有图片或文件引用（未更改原内容）",
            }
        }
    }
}

pub fn registered() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(clipboard_image::ClipboardImage),
        Arc::new(text::TextTool::Clean),
        Arc::new(text::TextTool::PowerShellPath),
        Arc::new(text::TextTool::RdpPath),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn summary(text: bool, image: bool, files: bool) -> ClipboardSummary {
        ClipboardSummary {
            sequence: 1,
            unicode_text: text,
            image,
            file_drop: files,
            virtual_file: false,
        }
    }

    #[test]
    fn descriptors_have_unique_stable_ids_and_structured_metadata() {
        let tools = registered();
        let ids = tools.iter().map(|tool| tool.id()).collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), tools.len());
        for tool in tools {
            let descriptor = tool.descriptor();
            assert!(!descriptor.id.is_empty());
            assert!(!descriptor.name.is_empty());
            assert!(!descriptor.accepted_inputs.is_empty());
            assert!(descriptor.side_effects.contains(&SideEffect::ClipboardRead));
            assert!(
                descriptor
                    .side_effects
                    .contains(&SideEffect::ClipboardWrite)
            );
            let value = serde_json::to_value(descriptor).unwrap();
            assert_eq!(value["id"], tool.id());
            assert!(value["accepted_inputs"].is_array());
        }
    }

    #[test]
    fn availability_is_derived_from_declared_input_kinds() {
        let tools = registered();
        assert!(
            tools[0]
                .availability(&summary(false, true, false))
                .is_available()
        );
        assert!(
            tools[0]
                .availability(&summary(false, false, true))
                .is_available()
        );
        let mut virtual_file = summary(false, false, false);
        virtual_file.virtual_file = true;
        assert!(tools[0].availability(&virtual_file).is_available());
        assert!(
            !tools[0]
                .availability(&summary(true, false, false))
                .is_available()
        );
        for tool in &tools[1..] {
            assert!(
                tool.availability(&summary(true, false, false))
                    .is_available()
            );
            assert!(
                !tool
                    .availability(&summary(false, true, false))
                    .is_available()
            );
        }
    }
}
