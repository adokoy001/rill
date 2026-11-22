use crate::{
    cli::{DataKind, Options},
    policy::charge,
    Failure, Result,
};
use std::{
    fs::File,
    io::{self, Read, Seek},
};
use ureq::{AsSendBody, SendBody};

const INPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct BodyBuffer {
    data: Vec<u8>,
    consumed: usize,
}

impl BodyBuffer {
    fn append(&mut self, bytes: &[u8], strip: bool) -> Result<()> {
        self.consumed = charge(self.consumed, bytes.len(), INPUT_LIMIT).ok_or_else(|| {
            Failure::new(
                26,
                "buffered upload exceeds 8 MiB; use --data-binary @file for streaming",
            )
        })?;
        self.data
            .try_reserve(bytes.len())
            .map_err(|e| Failure::new(26, e))?;
        if strip {
            self.data.extend(
                bytes
                    .iter()
                    .copied()
                    .filter(|b| !matches!(b, 0 | b'\r' | b'\n')),
            );
        } else {
            self.data.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn read_from(&mut self, mut input: impl Read, strip: bool) -> Result<()> {
        let mut block = [0u8; 8192];
        loop {
            let n = match input.read(&mut block) {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Failure::new(26, e)),
            };
            self.append(&block[..n], strip)?;
        }
    }
}

pub enum Payload {
    Empty(()),
    Bytes(Vec<u8>),
    File(File),
}

impl Payload {
    pub fn load(opts: &Options) -> Result<Self> {
        if opts.data.is_empty() {
            return Ok(Self::Empty(()));
        }
        let mut buffer = BodyBuffer::default();
        for (kind, data) in &opts.data {
            if !buffer.data.is_empty() && *kind != DataKind::Json {
                buffer.append(b"&", false)?;
            }
            let strip = *kind == DataKind::Form;
            match data.strip_prefix('@').filter(|_| *kind != DataKind::Raw) {
                Some("-") => buffer.read_from(io::stdin().lock(), strip)?,
                Some(path) => {
                    let file = File::open(path).map_err(|e| Failure::new(26, e))?;
                    if opts.data.len() == 1
                        && matches!(kind, DataKind::Binary | DataKind::Json)
                        && file.metadata().map_err(|e| Failure::new(26, e))?.is_file()
                    {
                        return Ok(Self::File(file));
                    }
                    buffer.read_from(file, strip)?;
                }
                None => buffer.append(data.as_bytes(), false)?,
            }
        }
        Ok(Self::Bytes(buffer.data))
    }

    pub fn as_body(&mut self) -> Result<SendBody<'_>> {
        match self {
            Self::Empty(unit) => Ok(unit.as_body()),
            Self::Bytes(bytes) => Ok(bytes.as_body()),
            Self::File(file) => {
                file.rewind().map_err(|e| Failure::new(26, e))?;
                Ok(file.as_body())
            }
        }
    }
}
