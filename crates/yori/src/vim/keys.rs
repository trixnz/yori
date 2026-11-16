//! The supported command vocabulary, not a general-purpose Vim keymap.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Motion {
    Left,
    Right,
    Up,
    Down,
    Word,
    BackWord,
    WordEnd,
    /// Whitespace-delimited WORD motions: `W`, `B` and `E`.
    BigWord,
    BigBackWord,
    BigWordEnd,
    Home,
    First,
    End,
    Line(Option<usize>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operator {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Motion(Motion),
    Lines,
    InnerWord,
    Selection,
    /// Visual `Y`: every line the selection touches, regardless of Visual mode.
    SelectedLines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Insert {
    Here,
    After,
    First,
    End,
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Move(Motion, usize),
    Operate(Operator, Target, usize),
    Insert(Insert),
    Visual(bool),
    DeleteChar(usize),
    Paste(bool, usize),
    Undo,
    Redo,
}

#[derive(Default)]
pub(super) struct Keys {
    count: Option<usize>,
    operator: Option<(Operator, usize)>,
    awaiting: Option<char>,
}

impl Keys {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn feed(&mut self, key: &str, visual: bool) -> Option<Command> {
        if let Some(awaiting) = self.awaiting.take() {
            let command = match (awaiting, key) {
                ('g', "g") => Some(self.motion(Motion::Line(Some(self.count.unwrap_or(1))))),
                ('i', "w") => self.operator.map(|(operator, count)| {
                    let count = (count * self.count.unwrap_or(1)).min(10_000);
                    Command::Operate(operator, Target::InnerWord, count)
                }),
                _ => None,
            };

            self.clear();

            return command;
        }

        if key.len() == 1
            && let Some(digit) = key.chars().next()?.to_digit(10)
            && (digit != 0 || self.count.is_some())
        {
            // Bound work for accidental long prefixes without overflow or UI lockups.
            self.count = Some((self.count.unwrap_or(0) * 10 + digit as usize).min(10_000));

            return None;
        }

        if key == "g" || (key == "i" && self.operator.is_some()) {
            self.awaiting = key.chars().next();

            return None;
        }

        let had_operator = self.operator.is_some();
        let command = self.command(key, visual);
        if had_operator || !matches!(key, "d" | "c" | "y") || command.is_some() {
            self.clear();
        }

        command
    }

    fn motion(&self, motion: Motion) -> Command {
        let count = self.count.unwrap_or(1);

        self.operator
            .map_or(Command::Move(motion, count), |(operator, prefix)| {
                Command::Operate(
                    operator,
                    Target::Motion(motion),
                    (count * prefix).min(10_000),
                )
            })
    }

    fn command(&mut self, key: &str, visual: bool) -> Option<Command> {
        let motion = match key {
            "h" | "left" => Some(Motion::Left),
            "l" | "right" => Some(Motion::Right),
            "k" | "up" => Some(Motion::Up),
            "j" | "down" => Some(Motion::Down),
            "w" => Some(Motion::Word),
            "b" => Some(Motion::BackWord),
            "e" => Some(Motion::WordEnd),
            "W" => Some(Motion::BigWord),
            "B" => Some(Motion::BigBackWord),
            "E" => Some(Motion::BigWordEnd),
            "0" | "home" => Some(Motion::Home),
            "^" => Some(Motion::First),
            "$" | "end" => Some(Motion::End),
            "G" => Some(Motion::Line(self.count)),
            _ => None,
        };
        if let Some(motion) = motion {
            return Some(self.motion(motion));
        }

        let operator = match key {
            "d" => Some(Operator::Delete),
            "c" => Some(Operator::Change),
            "y" => Some(Operator::Yank),
            _ => None,
        };
        if let Some(operator) = operator {
            if visual {
                return Some(Command::Operate(operator, Target::Selection, 1));
            }
            if let Some((pending, count)) = self.operator {
                return (pending == operator).then_some(Command::Operate(
                    operator,
                    Target::Lines,
                    (count * self.count.unwrap_or(1)).min(10_000),
                ));
            }

            self.operator = Some((operator, self.count.take().unwrap_or(1)));

            return None;
        }
        if self.operator.is_some() {
            return None;
        }

        let count = self.count.unwrap_or(1);

        match key {
            "i" => Some(Command::Insert(Insert::Here)),
            "a" => Some(Command::Insert(Insert::After)),
            "I" => Some(Command::Insert(Insert::First)),
            "A" => Some(Command::Insert(Insert::End)),
            "o" => Some(Command::Insert(Insert::Below)),
            "O" => Some(Command::Insert(Insert::Above)),
            "v" => Some(Command::Visual(false)),
            "V" => Some(Command::Visual(true)),
            "x" => Some(Command::DeleteChar(count)),
            "D" => Some(Command::Operate(
                Operator::Delete,
                Target::Motion(Motion::End),
                count,
            )),
            "C" => Some(Command::Operate(
                Operator::Change,
                Target::Motion(Motion::End),
                count,
            )),
            "Y" if visual => Some(Command::Operate(Operator::Yank, Target::SelectedLines, 1)),
            "Y" => Some(Command::Operate(Operator::Yank, Target::Lines, count)),
            "p" => Some(Command::Paste(true, count)),
            "P" => Some(Command::Paste(false, count)),
            "u" => Some(Command::Undo),
            "ctrl-r" => Some(Command::Redo),
            _ => None,
        }
    }
}
