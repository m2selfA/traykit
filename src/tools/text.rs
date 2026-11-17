//! Small text/path tools with pure transformations and a shared bounded clipboard adapter.
use super::{ActionCategory, InputKind, OutputKind, SideEffect, Tool, ToolDescriptor};
use crate::clipboard::{GlobalMemory, Owner, lock};
use anyhow::{Context, Result, ensure};
use windows::Win32::{
    Foundation::HGLOBAL,
    System::{
        DataExchange::{GetClipboardData, GetClipboardSequenceNumber},
        Memory::GlobalSize,
    },
};

const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const UNICODE_TEXT: u32 = 13;

pub enum TextTool {
    Clean,
    PowerShellPath,
    RdpPath,
}
impl Tool for TextTool {
    fn descriptor(&self) -> ToolDescriptor {
        let (id, name, description, category) = match self {
            Self::Clean => (
                "text-clean",
                "整理剪贴板文本",
                "去除每行末尾空格及制表符，统一 Windows 换行；保留缩进和空行",
                ActionCategory::Text,
            ),
            Self::PowerShellPath => (
                "path-powershell",
                "路径转 PowerShell 字符串",
                "将每行路径转成安全的单引号字符串；仅生成文本，不执行命令",
                ActionCategory::Path,
            ),
            Self::RdpPath => (
                "path-rdp",
                "RDP 路径互转",
                "每行绝对盘符路径与 \\\\tsclient\\盘符 路径互转；不访问远端",
                ActionCategory::Path,
            ),
        };
        ToolDescriptor {
            id,
            name,
            description,
            category,
            accepted_inputs: &[InputKind::Text],
            output: OutputKind::Text,
            side_effects: &[SideEffect::ClipboardRead, SideEffect::ClipboardWrite],
            supports_preview: false,
            supports_cancel: false,
        }
    }

    fn run(&self) -> Result<String> {
        transform_clipboard(|text| self.transform(text))?;
        Ok(match self {
            Self::Clean => "已整理剪贴板文本，可粘贴结果",
            Self::PowerShellPath => "已生成 PowerShell 安全路径字符串，可粘贴结果",
            Self::RdpPath => "已转换 Windows/RDP 路径，可粘贴结果",
        }
        .into())
    }
}
impl TextTool {
    pub fn transform(&self, input: &str) -> Result<String> {
        ensure!(!input.is_empty(), "剪贴板文本为空（未更改原内容）");
        match self {
            Self::Clean => Ok(input
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .split('\n')
                .map(|line| line.trim_end_matches([' ', '\t']))
                .collect::<Vec<_>>()
                .join("\r\n")),
            Self::PowerShellPath | Self::RdpPath => {
                let paths = paths(input)?;
                let output = paths
                    .into_iter()
                    .map(|path| match self {
                        Self::PowerShellPath => Ok(format!("'{}'", path.replace('\'', "''"))),
                        _ => rdp_path(path),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(output.join("\r\n"))
            }
        }
    }
}

fn paths(input: &str) -> Result<Vec<&str>> {
    let mut paths = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = if let Some(inner) = line.strip_prefix('"') {
            inner.strip_suffix('"').context("路径引号不成对")?
        } else {
            line
        };
        ensure!(
            !path.is_empty() && !path.chars().any(char::is_control) && !path.contains('"'),
            "路径为空或包含控制字符／双引号"
        );
        paths.push(path);
    }
    ensure!(!paths.is_empty(), "没有可转换的路径");
    Ok(paths)
}

fn rdp_path(path: &str) -> Result<String> {
    let normalized = path.replace('/', "\\");
    let prefix = r"\\tsclient\";
    if normalized
        .get(..prefix.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
    {
        let rest = &normalized[prefix.len()..];
        ensure!(
            rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && (rest.len() == 1 || rest.as_bytes().get(1) == Some(&b'\\')),
            "仅支持 tsclient 的单字母盘符共享"
        );
        let drive = rest.as_bytes()[0].to_ascii_uppercase() as char;
        let tail = rest.get(2..).unwrap_or("");
        return Ok(format!("{drive}:\\{tail}"));
    }
    let bytes = normalized.as_bytes();
    ensure!(
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\',
        "需要绝对盘符路径（如 C:\\目录）或 \\\\tsclient\\C\\目录"
    );
    Ok(format!(
        "{prefix}{}\\{}",
        bytes[0].to_ascii_uppercase() as char,
        &normalized[3..]
    ))
}

fn decode_text(bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() <= MAX_TEXT_BYTES && bytes.len().is_multiple_of(2),
        "Unicode 剪贴板数据超限或长度无效"
    );
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .context("Unicode 剪贴板数据缺少结束符")?;
    String::from_utf16(&units[..end]).context("Unicode 剪贴板数据无效")
}

fn transform_clipboard(transform: impl FnOnce(&str) -> Result<String>) -> Result<()> {
    let owner = Owner::new()?;
    let (input, sequence) = {
        let _guard = lock(owner.0)?;
        let handle = unsafe { GetClipboardData(UNICODE_TEXT) }
            .context("剪贴板中没有 Unicode 文本（未更改原内容）")?;
        let size = unsafe { GlobalSize(HGLOBAL(handle.0)) };
        ensure!(
            size > 0 && size <= MAX_TEXT_BYTES,
            "剪贴板文本超过 4 MiB 或为空"
        );
        let mut bytes = Vec::new();
        clipboard_win::raw::get_vec(UNICODE_TEXT, &mut bytes)
            .map_err(|e| anyhow::anyhow!("读取文本失败：{e}"))?;
        (decode_text(&bytes)?, unsafe {
            GetClipboardSequenceNumber()
        })
    };
    let output = transform(&input)?;
    if output == input {
        return Ok(());
    }
    ensure!(!output.contains('\0'), "输出包含 NUL，已取消写回");
    let bytes = output
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    ensure!(bytes.len() <= MAX_TEXT_BYTES, "转换后的文本超过 4 MiB");
    let mut memory = GlobalMemory::copy(&bytes)?;
    let _guard = lock(owner.0)?;
    ensure!(
        unsafe { GetClipboardSequenceNumber() } == sequence,
        "转换期间剪贴板已更改，取消写回并保留新内容"
    );
    clipboard_win::empty().map_err(|e| anyhow::anyhow!("清空剪贴板失败：{e}"))?;
    memory
        .publish(UNICODE_TEXT)
        .context("Windows 拒绝文本写入（剪贴板已清空）")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clean_preserves_indentation_and_blank_lines() {
        assert_eq!(
            TextTool::Clean
                .transform("  缩进 \t\r\n\r\nend\t\r")
                .unwrap(),
            "  缩进\r\n\r\nend\r\n"
        );
        assert_eq!(TextTool::Clean.transform(" \t").unwrap(), "");
    }
    #[test]
    fn powershell_literal_quotes_and_multiline() {
        assert_eq!(
            TextTool::PowerShellPath
                .transform("\"C:\\用户\\a b\"\r\nC:\\O'Brien\\$(cmd)\r\n")
                .unwrap(),
            "'C:\\用户\\a b'\r\n'C:\\O''Brien\\$(cmd)'"
        );
    }
    #[test]
    fn rdp_roundtrip_and_invalid_inputs() {
        for path in [r"C:\", r"D:\用户\a b.png", r"Z:\a'b"] {
            assert_eq!(rdp_path(&rdp_path(path).unwrap()).unwrap(), path);
        }
        assert_eq!(rdp_path(r"\\TSCLIENT\c").unwrap(), r"C:\");
        for path in [
            "",
            r"C:relative",
            r"\\server\share",
            r"\\tsclient\Documents\x",
            "👋",
        ] {
            assert!(rdp_path(path).is_err(), "{path}");
        }
        assert!(TextTool::RdpPath.transform("C:\\valid\nrelative").is_err());
    }
    #[test]
    fn malformed_and_unsupported_text_never_yields_output() {
        assert_eq!(decode_text(&[0x60, 0x4F, 0, 0]).unwrap(), "你");
        for bytes in [vec![0], vec![1, 0], vec![0, 0xD8, 0, 0]] {
            assert!(decode_text(&bytes).is_err());
        }
        assert!(TextTool::PowerShellPath.transform("\"unmatched").is_err());
        assert!(TextTool::PowerShellPath.transform("x\0y").is_err());
    }
}
