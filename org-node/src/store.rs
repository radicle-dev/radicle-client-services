use std::fs;
use std::io;
use std::path::Path;

/// State store.
#[derive(Debug)]
pub struct Store {
    /// Underlying file.
    file: fs::File,
    /// Deserialized state.
    pub state: State,
}

/// Node state.
#[derive(serde::Serialize, serde::Deserialize, Debug, Default)]
pub struct State {
    /// Timestamp of last successful sync.
    pub timestamp: u64,
}

impl Store {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .and_then(Self::from)
    }

    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;

        Ok(Self {
            state: State::default(),
            file,
        })
    }

    pub fn from(mut file: fs::File) -> io::Result<Self> {
        use io::Read;

        let mut s = String::new();
        file.read_to_string(&mut s)?;

        let state = if !s.is_empty() {
            let val = serde_json::from_str(&s)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            serde_json::from_value(val)?
        } else {
            State::default()
        };
        Ok(Self { file, state })
    }

    pub fn write(&mut self) -> io::Result<()> {
        use io::{Seek, Write};

        let s = serde_json::to_string(&self.state)?;

        self.file.set_len(0)?;
        self.file.seek(io::SeekFrom::Start(0))?;
        self.file.write_all(s.as_bytes())?;
        self.file.write_all(&[b'\n'])?;
        self.file.sync_data()?;

        Ok(())
    }
}
