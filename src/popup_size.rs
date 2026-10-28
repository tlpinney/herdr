use std::borrow::Cow;

use ratatui::layout::Rect;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupSize {
    Cells(u16),
    Percent(u8),
}

impl PopupSize {
    pub(crate) fn resolve(self, available: u16) -> u16 {
        match self {
            Self::Cells(cells) => cells,
            Self::Percent(percent) => ((available as u32 * percent as u32) / 100) as u16,
        }
    }

    pub(crate) fn parse_cli(value: &str) -> Result<Self, String> {
        if let Some(percent) = value.strip_suffix('%') {
            let percent = percent
                .parse::<u8>()
                .map_err(|_| "must be a number of cells or a percentage like 80%".to_string())?;
            if !(1..=100).contains(&percent) {
                return Err("percentage must be between 1% and 100%".to_string());
            }
            return Ok(Self::Percent(percent));
        }
        value
            .parse::<u16>()
            .map(Self::Cells)
            .map_err(|_| "must be a number of cells or a percentage like 80%".to_string())
    }

    fn parse_percent_string(value: &str) -> Result<Self, String> {
        if value.ends_with('%') {
            return Self::parse_cli(value);
        }
        Err("string sizes must be percentages like 80%; use a number for cells".to_string())
    }
}

/// Endpoint control carrying a cursor-anchored popup's outer rect, in
/// pane-surface cells (`[x, y, width, height]`), or `null` for a centered
/// popup.
pub const ANCHOR_CONTROL_KIND: &str = "herdr.popup_anchor.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupResolvedGeometry {
    pub outer: Rect,
    pub inner: Rect,
}

pub(crate) fn resolve_popup_geometry(
    width: Option<PopupSize>,
    height: Option<PopupSize>,
    area: Rect,
) -> Option<PopupResolvedGeometry> {
    let outer_width = popup_extent(width, area.width, 6);
    let outer_height = popup_extent(height, area.height, 4);
    if outer_width < 6 || outer_height < 4 {
        return None;
    }
    Some(popup_geometry_for_outer(Rect::new(
        area.x + (area.width.saturating_sub(outer_width)) / 2,
        area.y + (area.height.saturating_sub(outer_height)) / 2,
        outer_width,
        outer_height,
    )))
}

/// The requested popup extent along one axis, defaulting to half of
/// `available`, at least `minimum` and at most `available`.
fn popup_extent(size: Option<PopupSize>, available: u16, minimum: u16) -> u16 {
    size.map(|size| size.resolve(available))
        .unwrap_or(available.saturating_div(2).max(minimum))
        .max(minimum)
        .min(available)
}

/// The popup's border box and terminal area for an outer rect.
pub(crate) fn popup_geometry_for_outer(outer: Rect) -> PopupResolvedGeometry {
    let pane_inner_width = outer.width.saturating_sub(2);
    let pane_inner_height = outer.height.saturating_sub(2);
    let terminal_cols = if pane_inner_width <= 4 {
        pane_inner_width
    } else {
        pane_inner_width.saturating_sub(1)
    };
    PopupResolvedGeometry {
        outer,
        inner: Rect::new(
            outer.x.saturating_add(1),
            outer.y.saturating_add(1),
            terminal_cols,
            pane_inner_height,
        ),
    }
}

/// Smallest anchored popup worth showing; a pane that cannot hold one lends
/// the popup the whole area instead.
const ANCHORED_MIN_WIDTH: u16 = 24;
const ANCHORED_MIN_HEIGHT: u16 = 7;

/// The outer rect of a popup opened at `cursor` inside `pane` (both in
/// `area`'s coordinates). It sits on the rows just above the cursor, or just
/// below when there is no room above, with its content starting in the
/// cursor's column. It stays inside the pane, giving up rows or columns to
/// fit, and only a pane too small for a readable popup lets it use the whole
/// area. `None` when even the area is too small; the caller then centers.
pub(crate) fn resolve_anchored_popup(
    width: Option<PopupSize>,
    height: Option<PopupSize>,
    area: Rect,
    cursor: (u16, u16),
    pane: Rect,
) -> Option<Rect> {
    let pane = pane.intersection(area);
    anchored_in(width, height, cursor, pane).or_else(|| {
        (pane != area)
            .then(|| anchored_in(width, height, cursor, area))
            .flatten()
    })
}

fn anchored_in(
    width: Option<PopupSize>,
    height: Option<PopupSize>,
    (column, row): (u16, u16),
    bounds: Rect,
) -> Option<Rect> {
    if bounds.width < ANCHORED_MIN_WIDTH || !(bounds.y..bounds.bottom()).contains(&row) {
        return None;
    }
    let outer_width = popup_extent(width, bounds.width, ANCHORED_MIN_WIDTH);
    let wanted = popup_extent(height, bounds.height, ANCHORED_MIN_HEIGHT);
    let above = row - bounds.y;
    let below = bounds.bottom() - row - 1;
    let (y, outer_height) = if above >= wanted {
        (row - wanted, wanted)
    } else if below >= wanted {
        (row + 1, wanted)
    } else if above >= below {
        (bounds.y, above)
    } else {
        (row + 1, below)
    };
    if outer_height < ANCHORED_MIN_HEIGHT {
        return None;
    }
    let x = column
        .saturating_sub(1)
        .clamp(bounds.x, bounds.right() - outer_width);
    Some(Rect::new(x, y, outer_width, outer_height))
}

impl Serialize for PopupSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Cells(cells) => serializer.serialize_u16(*cells),
            Self::Percent(percent) => serializer.serialize_str(&format!("{percent}%")),
        }
    }
}

impl<'de> Deserialize<'de> for PopupSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PopupSizeVisitor;

        impl serde::de::Visitor<'_> for PopupSizeVisitor {
            type Value = PopupSize;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a cell count or percentage string like 80%")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let value =
                    u16::try_from(value).map_err(|_| E::custom("cell count must fit in u16"))?;
                Ok(PopupSize::Cells(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let value = u16::try_from(value)
                    .map_err(|_| E::custom("cell count must be between 0 and 65535"))?;
                Ok(PopupSize::Cells(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                PopupSize::parse_percent_string(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(PopupSizeVisitor)
    }
}

impl schemars::JsonSchema for PopupSize {
    fn schema_name() -> Cow<'static, str> {
        "PopupSize".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "oneOf": [
                {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 65535,
                    "description": "Outer popup size in terminal cells, including the border."
                },
                {
                    "type": "string",
                    "pattern": "^(100|[1-9][0-9]?)%$",
                    "description": "Outer popup size as a percentage of the terminal area, for example 80%."
                }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PopupSize;

    #[test]
    fn parses_cells_and_percent() {
        assert_eq!(PopupSize::parse_cli("120"), Ok(PopupSize::Cells(120)));
        assert_eq!(PopupSize::parse_cli("80%"), Ok(PopupSize::Percent(80)));
        assert_eq!(PopupSize::Percent(80).resolve(100), 80);
    }

    #[test]
    fn rejects_invalid_percent() {
        assert!(PopupSize::parse_cli("0%").is_err());
        assert!(PopupSize::parse_cli("101%").is_err());
        assert!(PopupSize::parse_cli("%").is_err());
    }

    #[test]
    fn string_deserialization_requires_percent() {
        assert!(serde_json::from_value::<PopupSize>(serde_json::json!("120")).is_err());
        assert_eq!(
            serde_json::from_value::<PopupSize>(serde_json::json!("80%")).unwrap(),
            PopupSize::Percent(80)
        );
    }

    #[test]
    fn serializes_percent_as_string() {
        assert_eq!(
            serde_json::to_value(PopupSize::Percent(80)).unwrap(),
            serde_json::json!("80%")
        );
        assert_eq!(
            serde_json::to_value(PopupSize::Cells(120)).unwrap(),
            serde_json::json!(120)
        );
    }

    #[test]
    fn resolves_requested_outer_size_and_inner_terminal_area() {
        let resolved = super::resolve_popup_geometry(
            Some(PopupSize::Percent(80)),
            Some(PopupSize::Percent(40)),
            ratatui::layout::Rect::new(0, 0, 100, 30),
        )
        .unwrap();
        assert_eq!(resolved.outer, ratatui::layout::Rect::new(10, 9, 80, 12));
        assert_eq!(resolved.inner, ratatui::layout::Rect::new(11, 10, 77, 10));
    }

    #[test]
    fn allows_full_terminal_outer_size() {
        let resolved = super::resolve_popup_geometry(
            Some(PopupSize::Percent(100)),
            Some(PopupSize::Percent(100)),
            ratatui::layout::Rect::new(4, 2, 100, 30),
        )
        .unwrap();

        assert_eq!(resolved.outer, ratatui::layout::Rect::new(4, 2, 100, 30));
        assert_eq!(resolved.inner, ratatui::layout::Rect::new(5, 3, 97, 28));
    }

    #[test]
    fn anchored_popup_opens_above_the_cursor_inside_its_pane() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let pane = ratatui::layout::Rect::new(60, 0, 60, 40);
        let outer = super::resolve_anchored_popup(
            Some(PopupSize::Cells(40)),
            Some(PopupSize::Cells(12)),
            area,
            (70, 30),
            pane,
        )
        .unwrap();
        assert_eq!(outer, ratatui::layout::Rect::new(69, 18, 40, 12));
        assert_eq!(super::popup_geometry_for_outer(outer).inner.x, 70);
    }

    #[test]
    fn anchored_popup_goes_below_when_there_is_no_room_above() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let pane = ratatui::layout::Rect::new(0, 20, 120, 20);
        let outer = super::resolve_anchored_popup(
            Some(PopupSize::Cells(40)),
            Some(PopupSize::Cells(12)),
            area,
            (0, 22),
            pane,
        )
        .unwrap();
        assert_eq!(outer, ratatui::layout::Rect::new(0, 23, 40, 12));
    }

    #[test]
    fn anchored_popup_shrinks_and_stays_inside_a_narrow_pane() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let pane = ratatui::layout::Rect::new(80, 10, 30, 16);
        let outer = super::resolve_anchored_popup(
            Some(PopupSize::Cells(64)),
            Some(PopupSize::Cells(16)),
            area,
            (108, 19),
            pane,
        )
        .unwrap();
        assert_eq!(outer, ratatui::layout::Rect::new(80, 10, 30, 9));
    }

    #[test]
    fn anchored_popup_uses_the_area_when_the_pane_is_too_small() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let pane = ratatui::layout::Rect::new(0, 30, 20, 4);
        let outer = super::resolve_anchored_popup(
            Some(PopupSize::Cells(40)),
            Some(PopupSize::Cells(12)),
            area,
            (5, 31),
            pane,
        )
        .unwrap();
        assert_eq!(outer, ratatui::layout::Rect::new(4, 19, 40, 12));
    }

    #[test]
    fn enforces_runtime_minimum_terminal_width() {
        let resolved = super::resolve_popup_geometry(
            Some(PopupSize::Cells(4)),
            None,
            ratatui::layout::Rect::new(0, 0, 80, 24),
        )
        .unwrap();
        assert_eq!(resolved.outer.width, 6);
        assert_eq!(resolved.inner.width, 4);

        assert!(
            super::resolve_popup_geometry(None, None, ratatui::layout::Rect::new(0, 0, 5, 24),)
                .is_none()
        );
    }
}
