use crate::{Failure, Result};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
};
use tempfile::NamedTempFile;

enum Sink<'a> {
    Stdout(io::StdoutLock<'static>),
    Direct(File),
    Staged(NamedTempFile, &'a Path),
}

impl Sink<'_> {
    fn writer(&mut self) -> &mut dyn Write {
        match self {
            Self::Stdout(w) => w,
            Self::Direct(w) => w,
            Self::Staged(w, _) => w,
        }
    }
}

pub fn write_error(e: io::Error) -> Failure {
    Failure::new(23, e)
}

fn check_destination(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic output requires a regular file or a new path",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub struct Output<'a> {
    path: Option<&'a Path>,
    atomic: bool,
    sink: Option<Sink<'a>>,
}

impl<'a> Output<'a> {
    pub fn new(path: Option<&'a str>, atomic: bool) -> Self {
        Self {
            path: path.filter(|&p| p != "-").map(Path::new),
            atomic,
            sink: None,
        }
    }
    // No filesystem operation is performed until the response actually needs a writer.
    pub fn writer(&mut self) -> Result<&mut dyn Write> {
        let sink = match self.sink {
            Some(ref mut open) => open,
            None => self.sink.insert(match self.path {
                None => Sink::Stdout(io::stdout().lock()),
                Some(path) if self.atomic => {
                    check_destination(path).map_err(write_error)?;
                    let directory = path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new("."));
                    let tmp = tempfile::Builder::new()
                        .prefix(".rill-")
                        .tempfile_in(directory)
                        .map_err(write_error)?;
                    Sink::Staged(tmp, path)
                }
                Some(path) => Sink::Direct(File::create(path).map_err(write_error)?),
            }),
        };
        Ok(sink.writer())
    }

    pub fn flush(&mut self) -> Result<()> {
        if let Some(sink) = self.sink.as_mut() {
            sink.writer().flush().map_err(write_error)?;
        }
        Ok(())
    }

    pub fn commit(self) -> Result<()> {
        if let Some(Sink::Staged(file, path)) = self.sink {
            file.as_file().sync_all().map_err(write_error)?;
            check_destination(path).map_err(write_error)?;
            file.persist(path).map_err(|e| write_error(e.error))?;
        }
        Ok(())
    }
}
