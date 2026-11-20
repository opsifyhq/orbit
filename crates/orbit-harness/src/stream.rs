use std::io;

use async_stream::stream;
use futures::Stream;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Keep a single harness record bounded even when a tool writes a large file or payload.
pub(crate) const MAX_LINE_BYTES: usize = 256 * 1024;

/// Leave room for the executable's other arguments on platforms with a small argv limit.
pub(crate) const MAX_PROMPT_ARG_BYTES: usize = 100 * 1024;

/// Timeline event payloads are intentionally smaller than the input stream limit.
pub(crate) const MAX_EVENT_DATA_BYTES: usize = 8 * 1024;

pub(crate) fn lines<R>(mut reader: R) -> impl Stream<Item = Result<String, io::Error>>
where
    R: AsyncRead + Unpin,
{
    stream! {
        let mut chunk = [0_u8; 8 * 1024];
        let mut line = Vec::with_capacity(chunk.len());
        let mut oversized = false;

        loop {
            let read = match reader.read(&mut chunk).await {
                Ok(read) => read,
                Err(error) => {
                    yield Err(error);
                    break;
                }
            };
            if read == 0 {
                if oversized {
                    yield Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line exceeded {} bytes", MAX_LINE_BYTES),
                    ));
                } else if !line.is_empty() {
                    yield decode_line(&mut line);
                }
                break;
            }

            for byte in &chunk[..read] {
                if *byte == b'\n' {
                    if oversized {
                        yield Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("line exceeded {} bytes", MAX_LINE_BYTES),
                        ));
                    } else {
                        yield decode_line(&mut line);
                    }
                    oversized = false;
                    continue;
                }
                if oversized {
                    continue;
                }
                if line.len() >= MAX_LINE_BYTES {
                    line.clear();
                    oversized = true;
                } else {
                    line.push(*byte);
                }
            }
        }
    }
}

fn decode_line(line: &mut Vec<u8>) -> Result<String, io::Error> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let decoded = String::from_utf8(std::mem::take(line)).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("line was not UTF-8: {error}"),
        )
    });
    line.clear();
    decoded
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[tokio::test]
    async fn oversized_line_is_reported_and_following_lines_are_read() {
        let (mut writer, reader) = tokio::io::duplex(MAX_LINE_BYTES + 32);
        writer
            .write_all(&vec![b'x'; MAX_LINE_BYTES + 1])
            .await
            .unwrap();
        writer.write_all(b"\nsmall\n").await.unwrap();
        writer.shutdown().await.unwrap();

        let reader = lines(reader);
        tokio::pin!(reader);
        assert!(reader.next().await.unwrap().is_err());
        assert_eq!(reader.next().await.unwrap().unwrap(), "small");
        assert!(reader.next().await.is_none());
    }
}
