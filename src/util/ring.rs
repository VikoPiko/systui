//! Bounded in-memory history.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct History {
    buf: VecDeque<f64>,
    cap: usize,
}

impl History {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            buf: VecDeque::with_capacity(cap.min(512)),
            cap,
        }
    }

    pub fn push(&mut self, v: f64) {
        if !v.is_finite() {
            return;
        }
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = f64> + '_ {
        self.buf.iter().copied()
    }

    pub fn to_vec(&self) -> Vec<f64> {
        self.buf.iter().copied().collect()
    }

    pub fn max(&self) -> Option<f64> {
        self.buf.iter().copied().reduce(f64::max)
    }
}
