//! The launcher prompt: a one-line command field, like the Quake console.
//!
//! `Prompt` is the editing logic and `Prompt::draw` the picture of it; neither
//! knows about keys or the renderer.

use crate::text::{Bitmap, GLYPH_SIZE, Rgba, text_width};

const BACKGROUND: Rgba = [20, 22, 28, 255];
const BORDER: Rgba = [90, 140, 200, 255];
const TEXT: Rgba = [235, 235, 235, 255];
const PROMPT_MARK: Rgba = [255, 180, 60, 255];

/// Commands remembered for Up to recall.
const HISTORY_LIMIT: usize = 100;

/// An editing key, already decoded from the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey {
    Char(char),
    Backspace,
    Up,
    Down,
    Enter,
    Escape,
}

/// What a key did that the caller has to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptEvent {
    /// Run this command.
    Run(String),
}

#[derive(Debug, Default, Clone)]
pub struct Prompt {
    open: bool,
    text: String,
    /// Oldest first.
    history: Vec<String>,
    /// Which entry `text` shows while recalling; `None` when typing.
    recalling: Option<usize>,
    /// What was being typed before Up recalled something.
    draft: String,
}

impl Prompt {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn open(&mut self) {
        self.open = true;
        self.clear_line();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.clear_line();
    }

    fn clear_line(&mut self) {
        self.text.clear();
        self.draft.clear();
        self.recalling = None;
    }

    /// Handle a key. Does nothing while the prompt is closed.
    pub fn key(&mut self, key: PromptKey) -> Option<PromptEvent> {
        if !self.open {
            return None;
        }
        match key {
            PromptKey::Char(ch) => {
                // Editing a recalled command turns it into a new one.
                self.recalling = None;
                self.text.push(ch);
            }
            PromptKey::Backspace => {
                self.recalling = None;
                self.text.pop();
            }
            PromptKey::Up => self.recall_older(),
            PromptKey::Down => self.recall_newer(),
            PromptKey::Escape => self.close(),
            PromptKey::Enter => {
                let command = self.text.trim().to_string();
                self.close();
                if command.is_empty() {
                    return None;
                }
                // Repeating the last command should not fill the history.
                if self.history.last() != Some(&command) {
                    self.history.push(command.clone());
                    if self.history.len() > HISTORY_LIMIT {
                        self.history.remove(0);
                    }
                }
                return Some(PromptEvent::Run(command));
            }
        }
        None
    }

    fn recall_older(&mut self) {
        let index = match self.recalling {
            Some(0) => return,
            Some(index) => index - 1,
            None if self.history.is_empty() => return,
            None => {
                self.draft = std::mem::take(&mut self.text);
                self.history.len() - 1
            }
        };
        self.recalling = Some(index);
        self.text = self.history[index].clone();
    }

    fn recall_newer(&mut self) {
        let Some(index) = self.recalling else {
            return;
        };
        if index + 1 < self.history.len() {
            self.recalling = Some(index + 1);
            self.text = self.history[index + 1].clone();
        } else {
            self.recalling = None;
            self.text = std::mem::take(&mut self.draft);
        }
    }

    /// How tall the picture is at `scale`.
    pub fn height(scale: u32) -> u32 {
        GLYPH_SIZE * scale + 2 * Self::padding(scale)
    }

    fn padding(scale: u32) -> u32 {
        4 * scale
    }

    /// Draw the prompt as a bar `width` pixels wide, its text `scale` times
    /// the font's size. A line too long to fit shows its end.
    pub fn draw(&self, width: u32, scale: u32) -> Bitmap {
        let pad = Self::padding(scale);
        let mut bitmap = Bitmap::new(width, Self::height(scale));
        bitmap.fill_rect(0, 0, width, bitmap.height, BORDER);
        let border = scale.min(width / 2).min(bitmap.height / 2);
        bitmap.fill_rect(
            border,
            border,
            width.saturating_sub(2 * border),
            bitmap.height.saturating_sub(2 * border),
            BACKGROUND,
        );

        let cell = text_width(1, scale);
        let mark_end = bitmap.draw_text(pad, pad, ">", scale, PROMPT_MARK);
        let text_x = mark_end + cell;
        // Room for the text and the cursor after it.
        let room = width.saturating_sub(text_x + pad) / cell;
        let room = room.saturating_sub(1) as usize;
        let count = self.text.chars().count();
        let shown: String = self.text.chars().skip(count.saturating_sub(room)).collect();
        let end = bitmap.draw_text(text_x, pad, &shown, scale, TEXT);
        bitmap.fill_rect(end, pad, cell, GLYPH_SIZE * scale, TEXT);
        bitmap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(prompt: &mut Prompt, text: &str) {
        for ch in text.chars() {
            prompt.key(PromptKey::Char(ch));
        }
    }

    fn run(prompt: &mut Prompt, command: &str) {
        prompt.open();
        type_str(prompt, command);
        assert_eq!(
            prompt.key(PromptKey::Enter),
            Some(PromptEvent::Run(command.into()))
        );
    }

    #[test]
    fn keys_do_nothing_while_closed() {
        let mut prompt = Prompt::default();
        assert_eq!(prompt.key(PromptKey::Char('a')), None);
        assert_eq!(prompt.text(), "");
        assert!(!prompt.is_open());
    }

    #[test]
    fn typing_and_backspace_edit_the_line() {
        let mut prompt = Prompt::default();
        prompt.open();
        type_str(&mut prompt, "foox");
        prompt.key(PromptKey::Backspace);
        assert_eq!(prompt.text(), "foo");
        for _ in 0..5 {
            prompt.key(PromptKey::Backspace);
        }
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn enter_runs_the_command_and_closes() {
        let mut prompt = Prompt::default();
        run(&mut prompt, "foot -e htop");
        assert!(!prompt.is_open());
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn an_empty_command_runs_nothing_but_closes() {
        let mut prompt = Prompt::default();
        prompt.open();
        type_str(&mut prompt, "   ");
        assert_eq!(prompt.key(PromptKey::Enter), None);
        assert!(!prompt.is_open());
        prompt.open();
        prompt.key(PromptKey::Up);
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn escape_closes_without_running_or_remembering() {
        let mut prompt = Prompt::default();
        prompt.open();
        type_str(&mut prompt, "rm");
        assert_eq!(prompt.key(PromptKey::Escape), None);
        assert!(!prompt.is_open());
        prompt.open();
        assert_eq!(prompt.text(), "");
        prompt.key(PromptKey::Up);
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn up_recalls_older_commands_and_down_comes_back() {
        let mut prompt = Prompt::default();
        run(&mut prompt, "one");
        run(&mut prompt, "two");
        prompt.open();
        type_str(&mut prompt, "dr");
        prompt.key(PromptKey::Up);
        assert_eq!(prompt.text(), "two");
        prompt.key(PromptKey::Up);
        assert_eq!(prompt.text(), "one");
        prompt.key(PromptKey::Up);
        assert_eq!(prompt.text(), "one");
        prompt.key(PromptKey::Down);
        assert_eq!(prompt.text(), "two");
        prompt.key(PromptKey::Down);
        assert_eq!(prompt.text(), "dr");
    }

    #[test]
    fn a_recalled_command_can_be_edited_and_run() {
        let mut prompt = Prompt::default();
        run(&mut prompt, "foot");
        prompt.open();
        prompt.key(PromptKey::Up);
        type_str(&mut prompt, " -F");
        assert_eq!(
            prompt.key(PromptKey::Enter),
            Some(PromptEvent::Run("foot -F".into()))
        );
    }

    #[test]
    fn repeating_a_command_keeps_one_entry() {
        let mut prompt = Prompt::default();
        run(&mut prompt, "foot");
        run(&mut prompt, "foot");
        prompt.open();
        prompt.key(PromptKey::Up);
        prompt.key(PromptKey::Up);
        assert_eq!(prompt.text(), "foot");
        prompt.key(PromptKey::Down);
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn the_picture_has_the_requested_size_and_shows_text() {
        let mut prompt = Prompt::default();
        prompt.open();
        let empty = prompt.draw(400, 2);
        assert_eq!((empty.width, empty.height), (400, Prompt::height(2)));
        type_str(&mut prompt, "hello");
        assert_ne!(prompt.draw(400, 2), empty);
    }

    #[test]
    fn a_long_line_still_draws_in_a_narrow_bar() {
        let mut prompt = Prompt::default();
        prompt.open();
        type_str(&mut prompt, &"x".repeat(200));
        prompt.draw(120, 2);
        prompt.draw(3, 2);
    }
}
