// src/terminal/scrollback.rs
use std::collections::VecDeque;

use crate::terminal::grid::Row;

pub struct ScrollBuffer {
    lines: VecDeque<Row>,
    max_lines: usize,
}

impl ScrollBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(max_lines),
            max_lines,
        }
    }

    pub fn push(&mut self, row: Row) {
        if self.lines.len() >= self.max_lines {
            self.lines.pop_front();
        }
        self.lines.push_back(row);
    }

    pub fn len(&self) -> usize { self.lines.len() }
    pub fn is_empty(&self) -> bool { self.lines.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = &Row> {
        self.lines.iter()
    }

    pub fn get(&self, idx: usize) -> Option<&Row> {
        self.lines.get(idx)
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}