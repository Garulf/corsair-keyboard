use std::io::{self, BufRead, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const MAX_LINE: usize = 1 << 20;

pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let mut line =
        serde_json::to_string(message).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

pub fn read_message<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut line = String::new();
    let read = reader.by_ref().take(MAX_LINE as u64 + 1).read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if read > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message longer than 1 MiB"));
    }
    serde_json::from_str(&line).map(Some).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::*;
    use crate::{Reply, Request};

    #[test]
    fn round_trips_one_message_per_line() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &Request::SetBrightness { percent: 40 }).unwrap();
        write_message(&mut buffer, &Request::GetState).unwrap();
        assert_eq!(String::from_utf8(buffer.clone()).unwrap().lines().count(), 2);
        let mut reader = BufReader::new(Cursor::new(buffer));
        assert_eq!(read_message::<_, Request>(&mut reader).unwrap(), Some(Request::SetBrightness { percent: 40 }));
        assert_eq!(read_message::<_, Request>(&mut reader).unwrap(), Some(Request::GetState));
        assert_eq!(read_message::<_, Request>(&mut reader).unwrap(), None);
    }

    #[test]
    fn rejects_garbage_and_oversized_lines() {
        let mut garbage = BufReader::new(Cursor::new(b"{not json}\n".to_vec()));
        assert_eq!(read_message::<_, Reply>(&mut garbage).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        let huge = vec![b'x'; MAX_LINE + 10];
        let mut oversized = BufReader::new(Cursor::new(huge));
        assert_eq!(read_message::<_, Reply>(&mut oversized).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }
}
