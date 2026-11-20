//! Batches: how many documents the extractor is given at a time, and how
//! many files one request stores.

use std::time::Duration;

use super::{File, GROWTH, MAX_BATCH, SENT_BYTES, SENT_DOCUMENTS};

/// How many documents the next batch is given: as many as take `target`
/// at the pace of the batch before, which read `read` documents in `took`.
/// It is at most `GROWTH` times as many, so that one quick batch does not
/// make the next one too long.
pub(super) fn next_batch(read: u64, took: Duration, target: Duration) -> u64 {
    let fits = target.as_secs_f64() * read as f64 / took.as_secs_f64().max(0.001);
    (fits as u64).clamp(1, (read * GROWTH).clamp(1, MAX_BATCH))
}

/// The files in groups that are each sent in one request.
pub(super) fn sending(files: &[File]) -> Vec<&[File]> {
    let mut groups = Vec::new();
    let (mut start, mut bytes) = (0, 0);
    for (index, file) in files.iter().enumerate() {
        if index > start
            && (index - start == SENT_DOCUMENTS || bytes + file.text.len() > SENT_BYTES)
        {
            groups.push(&files[start..index]);
            (start, bytes) = (index, 0);
        }
        bytes += file.text.len();
    }
    if start < files.len() {
        groups.push(&files[start..]);
    }
    groups
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{File, next_batch, sending};

    #[test]
    fn a_batch_is_made_to_take_the_time_it_is_given_and_grows_by_steps() {
        let target = Duration::from_secs(120);
        // Five documents in ten seconds: sixty fit, and twenty are allowed.
        assert_eq!(next_batch(5, Duration::from_secs(10), target), 20);
        // Twenty in a minute: forty fit.
        assert_eq!(next_batch(20, Duration::from_secs(60), target), 40);
        // A batch that took longer than the time gets one smaller.
        assert_eq!(next_batch(40, Duration::from_secs(240), target), 20);
        // However slow, one document; however quick, a thousand.
        assert_eq!(next_batch(5, Duration::from_secs(3600), target), 1);
        assert_eq!(next_batch(800, Duration::from_millis(1), target), 1_000);
        // A server that lets code run twenty-five seconds gives five.
        assert_eq!(
            next_batch(5, Duration::from_secs(5), Duration::from_secs(5)),
            5
        );
    }

    #[test]
    fn files_are_sent_a_thousand_or_sixteen_megabytes_at_a_time() {
        let file = |bytes: usize| File {
            path: String::new(),
            name: String::new(),
            text: "x".repeat(bytes),
        };
        let small: Vec<File> = (0..2_500).map(|_| file(10)).collect();
        let sizes: Vec<usize> = sending(&small).iter().map(|group| group.len()).collect();
        assert_eq!(sizes, vec![1_000, 1_000, 500]);
        let large: Vec<File> = (0..3).map(|_| file(10 * 1024 * 1024)).collect();
        let sizes: Vec<usize> = sending(&large).iter().map(|group| group.len()).collect();
        assert_eq!(sizes, vec![1, 1, 1]);
        assert!(sending(&[]).is_empty());
    }
}
