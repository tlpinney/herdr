//! Experimental smooth scroll regions relayed to the host terminal.
//!
//! A program in a pane declares a rectangle it scrolls one row per wheel
//! event with `ESC _ smooth;id;top;bottom;left;right;pos;max ESC \` (or the
//! legacy full-width `smooth;top;bottom;pos;max`, id 0), and clears it with
//! `smooth;off[;id]`. A host terminal that supports the protocol shifts the
//! rectangle by the sub-row remainder of precision scrolling. Herdr keeps the
//! latest declarations per pane, translates them into host-screen cells, and
//! re-declares them alongside each composed frame.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Regions a host terminal tracks at once (matches the reference host).
pub const MAX_HOST_REGIONS: usize = 8;

/// Regions retained per pane.
const MAX_PANE_REGIONS: usize = MAX_HOST_REGIONS;

/// One region in pane-viewport cells. `bottom` is the overscan row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRegion {
    pub top: u16,
    pub bottom: u16,
    pub left: u16,
    /// Inclusive; `u16::MAX` means the full pane width.
    pub right: u16,
    pub pos: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Set {
        id: u32,
        region: PaneRegion,
    },
    /// Clear one region, or all of the pane's regions when `None`.
    Clear(Option<u32>),
}

/// Parse a declaration payload (the bytes after `smooth;`).
pub fn parse(payload: &[u8]) -> Option<Command> {
    let text = std::str::from_utf8(payload).ok()?;
    let mut fields = text.split(';');
    if text == "off" || text.starts_with("off;") {
        fields.next();
        return match fields.next() {
            None => Some(Command::Clear(None)),
            Some(id) if fields.next().is_none() => Some(Command::Clear(Some(id.parse().ok()?))),
            Some(_) => None,
        };
    }
    let values = fields
        .map(|field| field.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    let (id, top, bottom, left, right, pos, max) = match values[..] {
        [top, bottom, pos, max] => (0, top, bottom, 0, u32::from(u16::MAX), pos, max),
        [id, top, bottom, left, right, pos, max] => (id, top, bottom, left, right, pos, max),
        _ => return None,
    };
    let narrow = |value: u32| u16::try_from(value).ok();
    let region = PaneRegion {
        top: narrow(top)?,
        bottom: narrow(bottom)?,
        left: narrow(left)?,
        right: narrow(right)?,
        pos,
        max,
    };
    (region.top < region.bottom && region.left <= region.right)
        .then_some(Command::Set { id, region })
}

/// The latest declared regions of one pane, by app-chosen id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneRegions {
    regions: BTreeMap<u32, PaneRegion>,
    revision: u64,
}

impl PaneRegions {
    /// Apply one declaration payload. Returns whether the regions changed.
    pub fn apply_payload(&mut self, payload: &[u8]) -> bool {
        let Some(command) = parse(payload) else {
            return false;
        };
        let changed = match command {
            Command::Set { id, region } => {
                if self.regions.len() >= MAX_PANE_REGIONS && !self.regions.contains_key(&id) {
                    return false;
                }
                self.regions.insert(id, region) != Some(region)
            }
            Command::Clear(Some(id)) => self.regions.remove(&id).is_some(),
            Command::Clear(None) => {
                let had = !self.regions.is_empty();
                self.regions.clear();
                had
            }
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    pub fn clear(&mut self) {
        if !self.regions.is_empty() {
            self.regions.clear();
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Increments whenever the declared regions change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, PaneRegion)> + '_ {
        self.regions.iter().map(|(&id, &region)| (id, region))
    }
}

/// A cell rectangle; `x`/`y` are the top-left cell, inclusive extents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRect {
    pub top: u16,
    pub bottom: u16,
    pub left: u16,
    pub right: u16,
}

/// A region translated into pane-surface cells, as sent to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRegion {
    pub pane_id: String,
    pub id: u32,
    pub rect: CellRect,
    pub pos: u32,
    pub max: u32,
}

/// Translate a pane region into the coordinate space whose origin is
/// `(origin_x, origin_y)` for a pane content area of `width` x `height`.
/// The region is clipped to the pane; regions that lose their overscan row
/// or all visible rows are dropped.
pub fn translate(
    region: PaneRegion,
    origin_x: u16,
    origin_y: u16,
    width: u16,
    height: u16,
) -> Option<CellRect> {
    if width == 0 || height < 2 || region.top >= height || region.left >= width {
        return None;
    }
    // Clipping the bottom would hide the overscan row and misalign content.
    if region.bottom >= height {
        return None;
    }
    let right = region.right.min(width - 1);
    Some(CellRect {
        top: origin_y.checked_add(region.top)?,
        bottom: origin_y.checked_add(region.bottom)?,
        left: origin_x.checked_add(region.left)?,
        right: origin_x.checked_add(right)?,
    })
}

/// Region id herdr uses for a pane's own scrollback, above any id a program
/// can declare.
pub const SCROLLBACK_REGION_ID: u32 = 1000;

/// Endpoint control kind of the region companion message.
pub const CONTROL_KIND: &str = "herdr.smooth_regions.v1";

/// Companion payload sent before the surface it describes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRegions {
    pub regions: Vec<SurfaceRegion>,
}

/// One region declared to the host terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostRegion {
    pub rect: CellRect,
    pub pos: u32,
    pub max: u32,
}

/// Assigns stable host ids to regions keyed by (pane, app id) and encodes
/// the declarations for each presented frame.
#[derive(Debug, Default)]
pub struct HostRegionIds {
    slots: [Option<(String, u32)>; MAX_HOST_REGIONS],
}

impl HostRegionIds {
    /// Encode declarations for `regions` (already in host cells), clearing
    /// host ids whose region went away. Keys beyond the host limit are
    /// skipped and fall back to row scrolling.
    pub fn encode(&mut self, regions: &[(String, u32, HostRegion)], out: &mut Vec<u8>) {
        use std::io::Write;

        for (slot, key) in self.slots.iter_mut().enumerate() {
            let keep = key
                .as_ref()
                .is_some_and(|(pane, id)| regions.iter().any(|(p, i, _)| p == pane && i == id));
            if key.is_some() && !keep {
                *key = None;
                let _ = write!(out, "\x1b_smooth;off;{slot}\x1b\\");
            }
        }
        for (pane, id, region) in regions {
            let slot = self
                .slots
                .iter()
                .position(|key| key.as_ref().is_some_and(|(p, i)| p == pane && i == id))
                .or_else(|| {
                    let free = self.slots.iter().position(Option::is_none)?;
                    self.slots[free] = Some((pane.clone(), *id));
                    Some(free)
                });
            let Some(slot) = slot else {
                continue;
            };
            let r = region.rect;
            let _ = write!(
                out,
                "\x1b_smooth;{slot};{};{};{};{};{};{}\x1b\\",
                r.top, r.bottom, r.left, r.right, region.pos, region.max
            );
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(top: u16, bottom: u16, left: u16, right: u16, pos: u32, max: u32) -> PaneRegion {
        PaneRegion {
            top,
            bottom,
            left,
            right,
            pos,
            max,
        }
    }

    #[test]
    fn parse_v2_legacy_and_clear() {
        assert_eq!(
            parse(b"3;1;20;2;40;5;100"),
            Some(Command::Set {
                id: 3,
                region: region(1, 20, 2, 40, 5, 100)
            })
        );
        assert_eq!(
            parse(b"1;30;0;80"),
            Some(Command::Set {
                id: 0,
                region: region(1, 30, 0, u16::MAX, 0, 80)
            })
        );
        assert_eq!(parse(b"off"), Some(Command::Clear(None)));
        assert_eq!(parse(b"off;7"), Some(Command::Clear(Some(7))));
        for bad in [
            &b"5;5;0;1"[..],
            b"0;1;5;9;4;0;1",
            b"1;2;3",
            b"1;x;3;4",
            b"off;1;2",
            b"0;1;99999;0;1",
        ] {
            assert_eq!(parse(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn pane_regions_track_changes_by_id() {
        let mut regions = PaneRegions::default();
        assert!(regions.apply_payload(b"1;0;10;0;39;0;50"));
        assert!(
            !regions.apply_payload(b"1;0;10;0;39;0;50"),
            "same declaration"
        );
        assert!(regions.apply_payload(b"1;0;10;0;39;1;50"), "scrolled");
        assert!(regions.apply_payload(b"2;0;10;40;79;0;50"));
        assert_eq!(regions.iter().count(), 2);
        assert!(regions.apply_payload(b"off;1"));
        assert_eq!(
            regions.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            vec![2]
        );
        let before = regions.revision();
        assert!(!regions.apply_payload(b"garbage"));
        assert_eq!(regions.revision(), before);
        regions.clear();
        assert!(regions.is_empty());
    }

    #[test]
    fn translate_offsets_and_clips_to_the_pane() {
        // Full-width legacy region in a 40x12 pane at (10, 3).
        assert_eq!(
            translate(region(1, 11, 0, u16::MAX, 0, 9), 10, 3, 40, 12),
            Some(CellRect {
                top: 4,
                bottom: 14,
                left: 10,
                right: 49
            })
        );
        // Overscan row outside the pane: dropped rather than misaligned.
        assert_eq!(translate(region(1, 12, 0, 5, 0, 9), 10, 3, 40, 12), None);
        // Region starting outside the pane.
        assert_eq!(translate(region(0, 5, 40, 50, 0, 9), 10, 3, 40, 12), None);
    }

    #[test]
    fn host_ids_are_stable_and_cleared_when_gone() {
        let rect = CellRect {
            top: 1,
            bottom: 5,
            left: 0,
            right: 9,
        };
        let host = |pos| HostRegion { rect, pos, max: 20 };
        let mut ids = HostRegionIds::default();
        let mut out = Vec::new();

        ids.encode(
            &[("a".into(), 0, host(0)), ("b".into(), 0, host(0))],
            &mut out,
        );
        assert_eq!(
            String::from_utf8(std::mem::take(&mut out)).unwrap(),
            "\x1b_smooth;0;1;5;0;9;0;20\x1b\\\x1b_smooth;1;1;5;0;9;0;20\x1b\\"
        );

        // "a" goes away: its slot is cleared and "b" keeps slot 1.
        ids.encode(&[("b".into(), 0, host(3))], &mut out);
        assert_eq!(
            String::from_utf8(std::mem::take(&mut out)).unwrap(),
            "\x1b_smooth;off;0\x1b\\\x1b_smooth;1;1;5;0;9;3;20\x1b\\"
        );

        ids.encode(&[], &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b_smooth;off;1\x1b\\");
        assert!(ids.is_empty());
    }

    #[test]
    fn host_ids_skip_regions_beyond_the_limit() {
        let rect = CellRect {
            top: 1,
            bottom: 5,
            left: 0,
            right: 9,
        };
        let regions: Vec<_> = (0..MAX_HOST_REGIONS as u32 + 2)
            .map(|id| {
                (
                    "p".to_string(),
                    id,
                    HostRegion {
                        rect,
                        pos: 0,
                        max: 1,
                    },
                )
            })
            .collect();
        let mut ids = HostRegionIds::default();
        let mut out = Vec::new();
        ids.encode(&regions, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches("\x1b_smooth;").count(), MAX_HOST_REGIONS);
    }
}
