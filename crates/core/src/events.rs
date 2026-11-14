//! The demo session's event log, `events.jsonl` (US-10.4, DR-5, DR-6): clicks, scrolls,
//! window changes, key combinations and the cursor, one JSON object per line, each on
//! the session clock. The platform hooks hand over [`Input`]s; typed text never reaches
//! this far: a key that types is only [`Key::Text`], and a run of them is logged as its
//! length (DR-6).

use std::io::{self, Write};

use serde::Serialize;

/// The log's file name in a package.
pub const EVENTS_FILE: &str = "events.jsonl";

/// What the platform saw. Positions are in physical pixels, desktop coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Cursor {
        x: i32,
        y: i32,
    },
    Button {
        button: Button,
        down: bool,
        x: i32,
        y: i32,
    },
    /// In wheel notches; `dy` > 0 scrolls up, `dx` > 0 right.
    Scroll {
        dx: f32,
        dy: f32,
        x: i32,
        y: i32,
    },
    /// The active window changed, or its title did.
    Window {
        title: String,
        app: String,
    },
    Key(Key),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// A key press, as the platform classed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A key that types a character: which one is never kept.
    Text,
    /// Backspace or Delete with no Ctrl, Alt or Win: part of the typing around it.
    Erase,
    /// Anything else, named with its modifiers: `Ctrl+Shift+S`, `Enter`, `F5`.
    Named(String),
}

/// One line of `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Event<'a> {
    Cursor {
        x: i32,
        y: i32,
    },
    Click {
        button: Button,
        x: i32,
        y: i32,
    },
    Release {
        button: Button,
        x: i32,
        y: i32,
    },
    Scroll {
        dx: f32,
        dy: f32,
        x: i32,
        y: i32,
    },
    Window {
        title: &'a str,
        app: &'a str,
    },
    Key {
        keys: &'a str,
    },
    /// Typing: how many characters, from `t_ms` to `until_ms`, never which.
    Text {
        chars: u32,
        until_ms: u64,
    },
}

#[derive(Serialize)]
struct Line<'a> {
    t_ms: u64,
    #[serde(flatten)]
    event: Event<'a>,
}

struct TextRun {
    from_ms: u64,
    until_ms: u64,
    chars: u32,
}

/// Writes `events.jsonl` in time order.
pub struct EventLog<W: Write> {
    out: W,
    text: Option<TextRun>,
    cursor: Option<(i32, i32)>,
    window: Option<(String, String)>,
}

impl<W: Write> EventLog<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            text: None,
            cursor: None,
            window: None,
        }
    }

    /// Log `input`, seen at `t_ms`. The cursor is logged when it moved, a window when it
    /// changed; typing is held until something else happens, then logged as one run.
    pub fn push(&mut self, t_ms: u64, input: Input) -> io::Result<()> {
        match input {
            Input::Key(Key::Text) => {
                let run = self.text.get_or_insert(TextRun {
                    from_ms: t_ms,
                    until_ms: t_ms,
                    chars: 0,
                });
                run.until_ms = t_ms;
                run.chars += 1;
                return Ok(());
            }
            Input::Key(Key::Erase) if self.text.is_some() => {
                if let Some(run) = &mut self.text {
                    run.until_ms = t_ms;
                }
                return Ok(());
            }
            Input::Cursor { x, y } if self.cursor == Some((x, y)) => return Ok(()),
            _ => {}
        }
        self.end_text()?;
        let event = match &input {
            Input::Cursor { x, y } => {
                self.cursor = Some((*x, *y));
                Event::Cursor { x: *x, y: *y }
            }
            &Input::Button { button, down, x, y } if down => Event::Click { button, x, y },
            &Input::Button { button, x, y, .. } => Event::Release { button, x, y },
            &Input::Scroll { dx, dy, x, y } => Event::Scroll { dx, dy, x, y },
            Input::Window { title, app } => {
                let window = (title.clone(), app.clone());
                if self.window.as_ref() == Some(&window) {
                    return Ok(());
                }
                self.window = Some(window);
                Event::Window { title, app }
            }
            Input::Key(Key::Erase) => Event::Key { keys: "Backspace" },
            Input::Key(Key::Named(keys)) => Event::Key { keys },
            Input::Key(Key::Text) => unreachable!("handled above"),
        };
        self.write(t_ms, event)
    }

    /// Log the typing still held, and flush. Returns the writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.end_text()?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn end_text(&mut self) -> io::Result<()> {
        match self.text.take() {
            Some(run) => self.write(
                run.from_ms,
                Event::Text {
                    chars: run.chars,
                    until_ms: run.until_ms,
                },
            ),
            None => Ok(()),
        }
    }

    fn write(&mut self, t_ms: u64, event: Event) -> io::Result<()> {
        serde_json::to_writer(&mut self.out, &Line { t_ms, event })?;
        self.out.write_all(b"\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(inputs: Vec<(u64, Input)>) -> Vec<serde_json::Value> {
        let mut log = EventLog::new(Vec::new());
        for (t, input) in inputs {
            log.push(t, input).unwrap();
        }
        let out = log.finish().unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn typing_is_logged_as_its_length_only() {
        let text = |t| (t, Input::Key(Key::Text));
        let lines = log(vec![
            (0, Input::Key(Key::Named("Ctrl+L".into()))),
            text(100),
            text(150),
            (180, Input::Key(Key::Erase)),
            text(200),
            (260, Input::Key(Key::Named("Enter".into()))),
            text(900),
        ]);
        assert_eq!(
            lines,
            [
                serde_json::json!({"t_ms": 0, "type": "key", "keys": "Ctrl+L"}),
                serde_json::json!({"t_ms": 100, "type": "text", "chars": 3, "until_ms": 200}),
                serde_json::json!({"t_ms": 260, "type": "key", "keys": "Enter"}),
                serde_json::json!({"t_ms": 900, "type": "text", "chars": 1, "until_ms": 900}),
            ]
        );
    }

    #[test]
    fn clicks_scrolls_windows_and_the_cursor_when_it_moves() {
        let lines = log(vec![
            (0, Input::Cursor { x: 10, y: 20 }),
            (33, Input::Cursor { x: 10, y: 20 }),
            (66, Input::Cursor { x: 12, y: 20 }),
            (
                70,
                Input::Button {
                    button: Button::Left,
                    down: true,
                    x: 12,
                    y: 20,
                },
            ),
            (
                90,
                Input::Button {
                    button: Button::Left,
                    down: false,
                    x: 12,
                    y: 20,
                },
            ),
            (
                120,
                Input::Scroll {
                    dx: 0.0,
                    dy: -1.0,
                    x: 12,
                    y: 20,
                },
            ),
            (
                200,
                Input::Window {
                    title: "Inbox".into(),
                    app: "mail.exe".into(),
                },
            ),
            (
                210,
                Input::Window {
                    title: "Inbox".into(),
                    app: "mail.exe".into(),
                },
            ),
            (220, Input::Key(Key::Erase)),
        ]);
        let types: Vec<_> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            ["cursor", "cursor", "click", "release", "scroll", "window", "key"]
        );
        assert_eq!(lines[2]["button"], "left");
        assert_eq!(lines[4]["dy"], -1.0);
        assert_eq!(lines[5]["app"], "mail.exe");
        assert_eq!(lines[6]["keys"], "Backspace");
    }
}
