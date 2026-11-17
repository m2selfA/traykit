use anyhow::{Result, bail, ensure};
use std::{fmt, str::FromStr};
use windows::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Shortcut {
    pub modifiers: u32,
    pub key: u32,
}

impl FromStr for Shortcut {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let mut modifiers = 0;
        let mut key = None;
        for part in value.split('+').map(str::trim) {
            let upper = part.to_ascii_uppercase();
            let flag = match upper.as_str() {
                "ALT" => Some(1),
                "CTRL" | "CONTROL" => Some(2),
                "SHIFT" => Some(4),
                "WIN" | "SUPER" => Some(8),
                _ => None,
            };
            if let Some(flag) = flag {
                ensure!(modifiers & flag == 0, "Repeated modifier in {value}");
                modifiers |= flag;
                continue;
            }
            ensure!(
                key.is_none(),
                "Only one non-modifier key is allowed: {value}"
            );
            let vk = if upper.len() == 1 && upper.as_bytes()[0].is_ascii_alphanumeric() {
                u32::from(upper.as_bytes()[0])
            } else if let Some(number) = upper
                .strip_prefix('F')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=24).contains(n))
            {
                0x70 + number - 1
            } else {
                match upper.as_str() {
                    "SPACE" => 0x20,
                    "HOME" => 0x24,
                    "END" => 0x23,
                    "INSERT" => 0x2D,
                    "DELETE" => 0x2E,
                    _ => bail!("Unsupported shortcut key: {part}"),
                }
            };
            key = Some(vk);
        }
        ensure!(
            modifiers != 0,
            "Use at least one modifier (Ctrl, Alt, Shift, Win)"
        );
        Ok(Self {
            modifiers,
            key: key.ok_or_else(|| anyhow::anyhow!("Shortcut has no key: {value}"))?,
        })
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers & 2 != 0 {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers & 1 != 0 {
            parts.push("Alt".to_string());
        }
        if self.modifiers & 4 != 0 {
            parts.push("Shift".to_string());
        }
        if self.modifiers & 8 != 0 {
            parts.push("Win".to_string());
        }
        let key = match self.key {
            key if (b'0' as u32..=b'9' as u32).contains(&key)
                || (b'A' as u32..=b'Z' as u32).contains(&key) =>
            {
                char::from_u32(key).ok_or(fmt::Error)?.to_string()
            }
            0x70..=0x87 => format!("F{}", self.key - 0x70 + 1),
            0x20 => "Space".into(),
            0x24 => "Home".into(),
            0x23 => "End".into(),
            0x2D => "Insert".into(),
            0x2E => "Delete".into(),
            _ => return Err(fmt::Error),
        };
        parts.push(key);
        f.write_str(&parts.join("+"))
    }
}

/// Owns one real OS registration; failed replacements leave the old binding alive.
pub struct Registration {
    hwnd: HWND,
    id: i32,
}
impl Registration {
    pub fn new(hwnd: HWND, id: i32, shortcut: Shortcut) -> Result<Self> {
        unsafe {
            RegisterHotKey(
                Some(hwnd),
                id,
                HOT_KEY_MODIFIERS(shortcut.modifiers) | MOD_NOREPEAT,
                shortcut.key,
            )
        }
        .map_err(|e| anyhow::anyhow!("Cannot register global hotkey (in use or reserved): {e}"))?;
        Ok(Self { hwnd, id })
    }
    pub fn id(&self) -> i32 {
        self.id
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), self.id);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Menu,
    Tool(usize),
}

/// Retain every old OS registration until all replacements and persistence succeed.
#[derive(Default)]
pub struct Bindings {
    registrations: std::collections::BTreeMap<Shortcut, (Registration, Target)>,
}

pub struct PendingBindings {
    wanted: std::collections::BTreeMap<Shortcut, Target>,
    added: std::collections::BTreeMap<Shortcut, Registration>,
}

impl Bindings {
    pub fn prepare(&self, hwnd: HWND, wanted: Vec<(Shortcut, Target)>) -> Result<PendingBindings> {
        let mut pending = PendingBindings {
            wanted: Default::default(),
            added: Default::default(),
        };
        for (shortcut, target) in wanted {
            ensure!(
                pending.wanted.insert(shortcut, target).is_none(),
                "Duplicate shortcut in binding plan"
            );
            if self.registrations.contains_key(&shortcut) {
                continue;
            }
            let id = (1..=0xBFFF)
                .find(|id| {
                    !self.registrations.values().any(|(r, _)| r.id() == *id)
                        && !pending.added.values().any(|r| r.id() == *id)
                })
                .ok_or_else(|| anyhow::anyhow!("No free hotkey registration IDs"))?;
            pending
                .added
                .insert(shortcut, Registration::new(hwnd, id, shortcut)?);
        }
        Ok(pending)
    }

    pub fn commit(
        &mut self,
        mut pending: PendingBindings,
    ) -> std::collections::BTreeMap<usize, (Shortcut, Target)> {
        let mut next = std::collections::BTreeMap::new();
        let mut routes = std::collections::BTreeMap::new();
        for (shortcut, target) in pending.wanted {
            let registration = self
                .registrations
                .remove(&shortcut)
                .map(|(r, _)| r)
                .or_else(|| pending.added.remove(&shortcut))
                .expect("prepared registration");
            routes.insert(registration.id() as usize, (shortcut, target));
            next.insert(shortcut, (registration, target));
        }
        self.registrations = next;
        routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staged_os_bindings_swap_fail_and_release_without_clipboard_access() {
        let owner = crate::clipboard::Owner::new().unwrap();
        let mut bindings = Bindings::default();
        // Reserve available uncommon keys rather than assuming the user's shortcuts are free.
        let mut held = Vec::new();
        for key in 0x7C..=0x87 {
            let shortcut = Shortcut { modifiers: 3, key };
            if let Ok(registration) = Registration::new(owner.0, 1000 + key as i32, shortcut) {
                held.push((shortcut, registration));
                if held.len() == 4 {
                    break;
                }
            }
        }
        assert_eq!(
            held.len(),
            4,
            "Need four unused Ctrl+Alt+F13..F24 shortcuts for OS binding test"
        );
        let (blocked, blocker) = held.pop().unwrap();
        let (c, c_guard) = held.pop().unwrap();
        let (b, b_guard) = held.pop().unwrap();
        let (a, a_guard) = held.pop().unwrap();
        drop((a_guard, b_guard, c_guard));
        let routes = bindings.commit(
            bindings
                .prepare(owner.0, vec![(a, Target::Menu), (b, Target::Tool(0))])
                .unwrap(),
        );
        assert_eq!(routes.len(), 2);
        // Swaps reuse OS registrations and only change their dispatch targets.
        let routes = bindings.commit(
            bindings
                .prepare(owner.0, vec![(b, Target::Menu), (a, Target::Tool(1))])
                .unwrap(),
        );
        assert!(
            routes
                .values()
                .any(|(_, target)| *target == Target::Tool(1))
        );
        // c is staged first, blocked fails second: c must be released; a,b must survive.
        let mut wanted = vec![(c, Target::Menu), (blocked, Target::Tool(2))];
        assert!(bindings.prepare(owner.0, wanted.clone()).is_err());
        let c_probe = Registration::new(owner.0, 2001, c).unwrap();
        assert!(Registration::new(owner.0, 2002, a).is_err());
        assert!(Registration::new(owner.0, 2003, b).is_err());
        drop((c_probe, blocker));
        // Dropping a prepared change simulates persistence failure: all originals remain.
        wanted.remove(1);
        drop(bindings.prepare(owner.0, wanted).unwrap());
        let _probe = Registration::new(owner.0, 2004, c).unwrap();
        assert!(Registration::new(owner.0, 2005, a).is_err());
        drop(bindings);
        let _released_a = Registration::new(owner.0, 2006, a).unwrap();
        let _released_b = Registration::new(owner.0, 2007, b).unwrap();
    }

    #[test]
    fn modifier_order_and_case_are_normalized() {
        assert_eq!(
            "Ctrl+Alt+Q".parse::<Shortcut>().unwrap(),
            "alt + control + q".parse().unwrap()
        );
        assert_eq!("Win+F24".parse::<Shortcut>().unwrap().key, 0x87);
    }
    #[test]
    fn rejects_ambiguous_or_unsafe_bindings() {
        for value in [
            "",
            "Q",
            "Ctrl",
            "Ctrl+Ctrl+Q",
            "Ctrl+Q+E",
            "Alt+F25",
            "Ctrl++",
            "Ctrl+🐱",
        ] {
            assert!(value.parse::<Shortcut>().is_err(), "{value}");
        }
    }
}
