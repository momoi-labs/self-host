//! Remove the launch's Variable values before native output reaches s6-log.
//! Prefix buffering covers a value split across reads without delaying other output.

#[derive(Clone)]
pub(crate) struct Redactor {
    values: Vec<Vec<u8>>,
    pending: Vec<u8>,
}

impl Redactor {
    pub(crate) fn new(environment: &[(String, String)]) -> Self {
        let mut values: Vec<Vec<u8>> = environment
            .iter()
            .map(|(_, value)| value.as_bytes().to_vec())
            .filter(|value| !value.is_empty())
            .collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        Self {
            values,
            pending: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8], end: bool) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        let mut at = 0;
        while at < self.pending.len() {
            let remaining = &self.pending[at..];
            if !end
                && self
                    .values
                    .iter()
                    .any(|value| value.len() > remaining.len() && value.starts_with(remaining))
            {
                break;
            }
            if let Some(value) = self
                .values
                .iter()
                .find(|value| remaining.starts_with(value))
            {
                output.extend_from_slice(b"[redacted]");
                at += value.len();
            } else {
                output.push(self.pending[at]);
                at += 1;
            }
        }
        self.pending.drain(..at);
        output
    }

    /// Merge pipe readers before matching, and flush only when all have closed.
    pub(crate) fn forward(
        mut self,
        chunks: std::sync::mpsc::Receiver<Vec<u8>>,
        mut output: impl std::io::Write,
    ) -> std::io::Result<()> {
        for bytes in chunks {
            output.write_all(&self.push(&bytes, false))?;
        }
        output.write_all(&self.push(&[], true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_split_values_and_longer_values_without_buffering_unrelated_lines() {
        let mut redactor = Redactor::new(&[
            ("A".into(), "token".into()),
            ("B".into(), "token-long".into()),
            ("EMPTY".into(), "".into()),
        ]);
        assert_eq!(redactor.push(b"started\n", false), b"started\n");
        assert_eq!(redactor.push(b"value: to", false), b"value: ");
        assert!(redactor.push(b"ken-lo", false).is_empty());
        assert_eq!(redactor.push(b"ng\n", false), b"[redacted]\n");
        assert_eq!(redactor.push(b"token", true), b"[redacted]");
        assert_eq!(redactor.push(b"to", true), b"to");
    }
    #[test]
    fn matches_short_and_overlapping_values_and_flushes_the_last_prefix() {
        let mut redactor = Redactor::new(&[
            ("A".into(), "aba".into()),
            ("B".into(), "bab".into()),
            ("C".into(), "1".into()),
        ]);
        assert_eq!(redactor.push(b"xabab1", false), b"x[redacted]b[redacted]");
        assert!(redactor.push(b"ab", false).is_empty());
        assert_eq!(redactor.pending.len(), 2);
        assert_eq!(redactor.push(b"", true), b"ab");
        assert!(redactor.pending.is_empty());
        let ordinary = vec![b'z'; 20_000];
        assert_eq!(redactor.push(&ordinary, false), ordinary);
        assert!(redactor.pending.is_empty());
    }

    #[test]
    fn combined_output_keeps_a_prefix_when_one_pipe_closes_and_flushes_at_final_eof() {
        let (stdout, chunks) = std::sync::mpsc::sync_channel(4);
        let stderr = stdout.clone();
        stdout.send(b"value: token-".to_vec()).unwrap();
        drop(stdout);
        stderr.send(b"long\nlast: token-".to_vec()).unwrap();
        drop(stderr);

        let mut log = Vec::new();
        Redactor::new(&[("TOKEN".into(), "token-long".into())])
            .forward(chunks, &mut log)
            .unwrap();
        assert_eq!(log, b"value: [redacted]\nlast: token-");
    }
}
