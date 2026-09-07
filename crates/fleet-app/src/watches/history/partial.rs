use std::collections::VecDeque;

const CHUNK_BYTES: usize = 4096;

/// UTF-8 chunks keep front trimming independent of the retained line's length.
#[derive(Debug, Default)]
pub(super) struct PartialLine {
    chunks: VecDeque<String>,
    head: usize,
    len: usize,
}

impl PartialLine {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push_str(&mut self, mut text: &str) {
        self.len += text.len();
        while !text.is_empty() {
            if let Some(tail) = self.chunks.back_mut() {
                let mut end = (CHUNK_BYTES - tail.len()).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                if end > 0 {
                    tail.push_str(&text[..end]);
                    text = &text[end..];
                    continue;
                }
            }
            let mut end = CHUNK_BYTES.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut chunk = String::with_capacity(end);
            chunk.push_str(&text[..end]);
            self.chunks.push_back(chunk);
            text = &text[end..];
        }
    }

    pub fn trim_front(&mut self, bytes: usize) {
        let mut remaining = bytes.min(self.len);
        while remaining > 0 {
            let Some(front) = self.chunks.front() else {
                break;
            };
            let available = front.len() - self.head;
            if remaining >= available {
                self.chunks.pop_front();
                self.head = 0;
                self.len -= available;
                remaining -= available;
            } else {
                let mut end = self.head + remaining;
                while !front.is_char_boundary(end) {
                    end += 1;
                }
                self.len -= end - self.head;
                self.head = end;
                remaining = 0;
            }
        }
    }

    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.len);
        for (index, chunk) in self.chunks.iter().enumerate() {
            text.push_str(if index == 0 {
                &chunk[self.head..]
            } else {
                chunk
            });
        }
        text
    }

    pub fn take_text(&mut self) -> String {
        if self.head == 0
            && self.chunks.len() == 1
            && let Some(text) = self.chunks.pop_front()
        {
            self.len = 0;
            return text;
        }
        let text = self.text();
        *self = Self::default();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_keeps_utf8_and_does_not_move_retained_chunks() {
        let mut line = PartialLine::default();
        line.push_str(&"é".repeat(CHUNK_BYTES));
        let retained = line.chunks[1].as_ptr();
        line.trim_front(CHUNK_BYTES + 1);
        assert_eq!(line.len(), CHUNK_BYTES - 2);
        assert_eq!(line.chunks[0].as_ptr(), retained);
        assert_eq!(line.text(), "é".repeat((CHUNK_BYTES - 2) / 2));
        line.push_str("end");
        assert!(line.take_text().ends_with("end"));
        assert!(line.is_empty());
    }

    #[test]
    fn tiny_appends_share_bounded_chunks() {
        let mut line = PartialLine::default();
        for _ in 0..CHUNK_BYTES * 2 {
            line.push_str("x");
        }
        assert_eq!(line.chunks.len(), 2);
        line.trim_front(CHUNK_BYTES * 2);
        assert!(line.chunks.is_empty());
        assert_eq!(line.len(), 0);
    }
}
