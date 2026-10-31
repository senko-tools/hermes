//! Panes and their saved layout (PLAN.md sections 15.1 and 17). The pane tree is stored in
//! `[[layout]]` as one table per window: a split is
//! `{ split = "vertical", ratio = 0.5, a = {…}, b = {…} }`, a pane is
//! `{ buffer = "channel", instance = "…", channel = "…", members = true }`,
//! `{ buffer = "settings" }` or `{ buffer = "empty" }`.

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use iced::widget::pane_grid::{self, Axis, Configuration, Node};

use crate::buffer::Buffer;

#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub buffer: Buffer,
    /// The member list is docked on the right.
    pub members: bool,
}

impl Pane {
    pub fn new(buffer: Buffer) -> Pane {
        Pane {
            buffer,
            members: false,
        }
    }
}

/// The layout as `[[layout]]` tables (one window).
pub fn save(state: &pane_grid::State<Pane>) -> Vec<toml::Table> {
    vec![node_table(state, state.layout())]
}

fn node_table(state: &pane_grid::State<Pane>, node: &Node) -> toml::Table {
    let mut table = toml::Table::new();
    match node {
        Node::Split {
            axis, ratio, a, b, ..
        } => {
            let axis = match axis {
                Axis::Vertical => "vertical",
                Axis::Horizontal => "horizontal",
            };
            table.insert("split".into(), axis.into());
            table.insert(
                "ratio".into(),
                toml::Value::Float((f64::from(*ratio) * 1000.0).round() / 1000.0),
            );
            table.insert("a".into(), node_table(state, a).into());
            table.insert("b".into(), node_table(state, b).into());
        }
        Node::Pane(pane) => {
            let Some(pane) = state.get(*pane) else {
                table.insert("buffer".into(), "empty".into());
                return table;
            };
            match &pane.buffer {
                Buffer::Empty => {
                    table.insert("buffer".into(), "empty".into());
                }
                Buffer::Settings => {
                    table.insert("buffer".into(), "settings".into());
                }
                Buffer::Channel { inst, channel } => {
                    table.insert("buffer".into(), "channel".into());
                    table.insert("instance".into(), inst.clone().into());
                    table.insert("channel".into(), channel.to_string().into());
                }
            }
            if pane.members {
                table.insert("members".into(), true.into());
            }
        }
    }
    table
}

/// The saved layout; `None` if there is none or it does not parse (one empty pane is used).
pub fn restore(layout: &[toml::Table]) -> Option<Configuration<Pane>> {
    layout.first().and_then(config_of)
}

fn config_of(table: &toml::Table) -> Option<Configuration<Pane>> {
    if let Some(split) = table.get("split").and_then(toml::Value::as_str) {
        let axis = match split {
            "vertical" => Axis::Vertical,
            "horizontal" => Axis::Horizontal,
            _ => return None,
        };
        let ratio = table.get("ratio").and_then(toml::Value::as_float)?;
        let a = config_of(table.get("a")?.as_table()?)?;
        let b = config_of(table.get("b")?.as_table()?)?;
        return Some(Configuration::Split {
            axis,
            ratio: (ratio as f32).clamp(0.05, 0.95),
            a: Box::new(a),
            b: Box::new(b),
        });
    }
    let buffer = match table.get("buffer").and_then(toml::Value::as_str)? {
        "settings" => Buffer::Settings,
        "channel" => {
            let inst: InstanceId = table.get("instance")?.as_str()?.to_owned();
            let channel = table.get("channel")?.as_str()?.parse::<u64>().ok()?;
            Buffer::Channel {
                inst,
                channel: Id(channel),
            }
        }
        _ => Buffer::Empty,
    };
    Some(Configuration::Pane(Pane {
        buffer,
        members: table
            .get("members")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: u64) -> Buffer {
        Buffer::Channel {
            inst: "h".into(),
            channel: Id(id),
        }
    }

    #[test]
    fn layouts_round_trip() {
        let (mut state, first) = pane_grid::State::new(Pane::new(channel(11)));
        let (right, _) = state
            .split(Axis::Vertical, first, Pane::new(Buffer::Settings))
            .unwrap();
        let mut members = Pane::new(channel(12));
        members.members = true;
        state.split(Axis::Horizontal, right, members).unwrap();

        let saved = save(&state);
        // Through the TOML text, as in config.toml.
        let text = toml::to_string(&toml::Table::from_iter([(
            "layout".to_owned(),
            toml::Value::Array(saved.iter().cloned().map(toml::Value::Table).collect()),
        )]))
        .unwrap();
        let parsed: toml::Table = toml::from_str(&text).unwrap();
        let tables: Vec<toml::Table> = parsed["layout"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_table().unwrap().clone())
            .collect();

        let restored = pane_grid::State::with_configuration(restore(&tables).unwrap());
        assert_eq!(save(&restored), saved);
        let mut buffers: Vec<_> = restored.iter().map(|(_, p)| p.clone()).collect();
        buffers.sort_by_key(|p| format!("{:?}", p.buffer));
        assert_eq!(buffers.len(), 3);
        assert!(buffers.iter().any(|p| p.buffer == channel(12) && p.members));
        assert!(buffers.iter().any(|p| p.buffer == Buffer::Settings));
    }

    #[test]
    fn broken_layouts_are_ignored() {
        assert!(restore(&[]).is_none());
        let bad: toml::Table = toml::from_str("split = \"diagonal\"\nratio = 0.5").unwrap();
        assert!(restore(&[bad]).is_none());
        let half: toml::Table =
            toml::from_str("split = \"vertical\"\nratio = 0.5\na = { buffer = \"empty\" }")
                .unwrap();
        assert!(restore(&[half]).is_none());
        let odd: toml::Table = toml::from_str("buffer = \"mystery\"").unwrap();
        assert!(matches!(
            restore(&[odd]),
            Some(Configuration::Pane(p)) if p == Pane::new(Buffer::Empty)
        ));
    }
}
