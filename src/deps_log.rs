use rapidhash::fast::RapidHashMap as HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const SIGNATURE: &[u8] = b"# ninjadeps\n";
const VERSION: u32 = 4;
const MAX_RECORD_SIZE: usize = (1 << 19) - 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepsEntry {
    pub mtime: u64,
    pub inputs: Vec<String>,
}

#[derive(Debug, Default)]
pub struct DepsLog {
    path: PathBuf,
    file: Option<fs::File>,
    invalidated: bool,
    recovered: bool,
    nodes: Vec<String>,
    ids: HashMap<String, u32>,
    entries: HashMap<String, DepsEntry>,
    total_dep_records: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParseError {
    Truncated(usize),
    Invalid,
}

impl DepsLog {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let mut log = Self {
            path,
            ..Self::default()
        };
        let data = match fs::read(&log.path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(log),
            #[cfg(unix)]
            Err(error) if error.kind() == io::ErrorKind::IsADirectory => Vec::new(),
            Err(error) => return Err(error),
        };
        match log.parse(&data) {
            Ok(()) => {
                if log.total_dep_records > 1_000 && log.total_dep_records > log.entries.len() * 3 {
                    let _ = log.recompact();
                }
            }
            Err(ParseError::Truncated(valid_length)) => {
                OpenOptions::new()
                    .write(true)
                    .open(&log.path)
                    .and_then(|file| file.set_len(valid_length as u64))?;
                log.recovered = true;
            }
            Err(ParseError::Invalid) => {
                log.invalidated = true;
                log.nodes.clear();
                log.ids.clear();
                log.entries.clear();
                log.total_dep_records = 0;
                let _ = fs::remove_file(&log.path);
            }
        }
        Ok(log)
    }

    pub fn was_invalidated(&self) -> bool {
        self.invalidated
    }

    pub fn was_recovered(&self) -> bool {
        self.recovered
    }

    pub fn get(&self, output: &str) -> Option<&DepsEntry> {
        self.entries.get(output)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &DepsEntry)> {
        self.entries
            .iter()
            .map(|(path, entry)| (path.as_str(), entry))
    }

    pub fn outputs_in_node_order(&self) -> impl Iterator<Item = &str> {
        self.nodes
            .iter()
            .filter(|path| self.entries.contains_key(path.as_str()))
            .map(String::as_str)
    }

    pub fn path_exists(&self) -> bool {
        self.path.exists()
    }

    pub fn first_reverse_dep(&self, input: &str) -> Option<&str> {
        self.nodes.iter().find_map(|node| {
            self.entries
                .get(node)
                .is_some_and(|entry| entry.inputs.iter().any(|candidate| candidate == input))
                .then_some(node.as_str())
        })
    }

    pub fn record(&mut self, output: &str, mtime: u64, inputs: &[String]) -> io::Result<()> {
        if self
            .entries
            .get(output)
            .is_some_and(|entry| entry.mtime == mtime && entry.inputs == inputs)
        {
            return Ok(());
        }
        let output_id = self.ensure_id(output)?;
        let mut input_ids = Vec::with_capacity(inputs.len());
        for input in inputs {
            input_ids.push(self.ensure_id(input)?);
        }
        let payload_size = 12usize
            .checked_add(
                input_ids
                    .len()
                    .checked_mul(4)
                    .ok_or_else(record_too_large)?,
            )
            .ok_or_else(record_too_large)?;
        if payload_size > MAX_RECORD_SIZE {
            return Err(record_too_large());
        }
        let mut record = Vec::with_capacity(payload_size + 4);
        record.extend_from_slice(&((payload_size as u32) | 0x8000_0000).to_le_bytes());
        record.extend_from_slice(&output_id.to_le_bytes());
        record.extend_from_slice(&(mtime as u32).to_le_bytes());
        record.extend_from_slice(&((mtime >> 32) as u32).to_le_bytes());
        for input_id in input_ids {
            record.extend_from_slice(&input_id.to_le_bytes());
        }
        let file = self.open_append()?;
        file.write_all(&record)?;
        file.flush()?;
        self.entries.insert(
            output.to_owned(),
            DepsEntry {
                mtime,
                inputs: inputs.to_vec(),
            },
        );
        Ok(())
    }

    pub fn recompact(&mut self) -> io::Result<()> {
        self.recompact_retain(|_| true)
    }

    pub fn recompact_retain(&mut self, mut is_live: impl FnMut(&str) -> bool) -> io::Result<()> {
        self.close()?;
        let entries = self
            .outputs_in_node_order()
            .filter(|output| is_live(output))
            .map(|output| (output.to_owned(), self.entries[output].clone()))
            .collect::<Vec<_>>();
        let mut temporary = self.path.as_os_str().to_os_string();
        temporary.push(".recompact");
        let temporary = PathBuf::from(temporary);
        let mut compact = Self {
            path: temporary.clone(),
            ..Self::default()
        };
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        for (output, entry) in entries {
            compact.record(&output, entry.mtime, &entry.inputs)?;
        }
        if compact.entries.is_empty() {
            compact.open_append()?;
        }
        compact.close()?;
        fs::rename(&temporary, &self.path)?;
        *self = Self::load(self.path.clone())?;
        Ok(())
    }

    fn parse(&mut self, data: &[u8]) -> Result<(), ParseError> {
        if data.len() < 16
            || &data[..12] != SIGNATURE
            || read_u32(data, 12).map_err(|_| ParseError::Invalid)? != VERSION
        {
            return Err(ParseError::Invalid);
        }
        let mut offset = 16;
        while offset < data.len() {
            let record_start = offset;
            let encoded_size =
                read_u32(data, offset).map_err(|_| ParseError::Truncated(record_start))?;
            offset += 4;
            let is_deps = encoded_size & 0x8000_0000 != 0;
            let size = (encoded_size & 0x7fff_ffff) as usize;
            if size == 0 || size > MAX_RECORD_SIZE {
                return Err(ParseError::Truncated(record_start));
            }
            if offset + size > data.len() {
                return Err(ParseError::Truncated(record_start));
            }
            let record = &data[offset..offset + size];
            if is_deps {
                self.parse_deps_record(record)
                    .map_err(|_| ParseError::Truncated(record_start))?;
            } else {
                self.parse_path_record(record)
                    .map_err(|_| ParseError::Truncated(record_start))?;
            }
            offset += size;
        }
        Ok(())
    }

    fn parse_path_record(&mut self, record: &[u8]) -> Result<(), ()> {
        if record.len() < 5 {
            return Err(());
        }
        let checksum = read_u32(record, record.len() - 4)?;
        let id = self.nodes.len() as u32;
        if checksum != !id {
            return Err(());
        }
        let mut path_end = record.len() - 4;
        for _ in 0..3 {
            if path_end > 0 && record[path_end - 1] == 0 {
                path_end -= 1;
            }
        }
        if path_end == 0 {
            return Err(());
        }
        let path = std::str::from_utf8(&record[..path_end]).map_err(|_| ())?;
        if self.ids.contains_key(path) {
            return Err(());
        }
        self.ids.insert(path.to_owned(), id);
        self.nodes.push(path.to_owned());
        Ok(())
    }

    fn parse_deps_record(&mut self, record: &[u8]) -> Result<(), ()> {
        if record.len() < 12 || !record.len().is_multiple_of(4) {
            return Err(());
        }
        let output_id = read_u32(record, 0)? as usize;
        let output = self.nodes.get(output_id).ok_or(())?.clone();
        let mtime = read_u32(record, 4)? as u64 | ((read_u32(record, 8)? as u64) << 32);
        let mut inputs = Vec::with_capacity(record.len() / 4 - 3);
        for offset in (12..record.len()).step_by(4) {
            let input_id = read_u32(record, offset)? as usize;
            inputs.push(self.nodes.get(input_id).ok_or(())?.clone());
        }
        self.total_dep_records += 1;
        self.entries.insert(output, DepsEntry { mtime, inputs });
        Ok(())
    }

    fn ensure_id(&mut self, path: &str) -> io::Result<u32> {
        if let Some(id) = self.ids.get(path) {
            return Ok(*id);
        }
        let id = self.nodes.len() as u32;
        let path_bytes = path.as_bytes();
        let padding = (4 - path_bytes.len() % 4) % 4;
        let payload_size = path_bytes.len() + padding + 4;
        if path_bytes.is_empty() || payload_size > MAX_RECORD_SIZE {
            return Err(record_too_large());
        }
        let mut record = Vec::with_capacity(payload_size + 4);
        record.extend_from_slice(&(payload_size as u32).to_le_bytes());
        record.extend_from_slice(path_bytes);
        record.extend_from_slice(&[0; 3][..padding]);
        record.extend_from_slice(&(!id).to_le_bytes());
        self.open_append()?.write_all(&record)?;
        self.ids.insert(path.to_owned(), id);
        self.nodes.push(path.to_owned());
        Ok(id)
    }

    fn open_append(&mut self) -> io::Result<&mut fs::File> {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
            let new_file = !self.path.exists() || self.path.metadata()?.len() == 0;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            if new_file {
                file.write_all(SIGNATURE)?;
                file.write_all(&VERSION.to_le_bytes())?;
            }
            self.file = Some(file);
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("dependency log file was not opened"))
    }

    fn close(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        Ok(())
    }
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, ()> {
    let bytes = data.get(offset..offset + 4).ok_or(())?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| ())?))
}

fn record_too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "dependency record is too large",
    )
}

pub fn deps_log_path(builddir: Option<&str>) -> PathBuf {
    builddir
        .filter(|directory| !directory.is_empty())
        .map_or_else(
            || PathBuf::from(".ninja_deps"),
            |directory| Path::new(directory).join(".ninja_deps"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_sample_log(path: &Path) -> Vec<u8> {
        let mut log = DepsLog::load(path.to_owned()).unwrap();
        log.record("out.o", 1, &["foo.h".to_owned(), "bar.h".to_owned()])
            .unwrap();
        log.record("out2.o", 2, &["foo.h".to_owned(), "bar2.h".to_owned()])
            .unwrap();
        drop(log);
        fs::read(path).unwrap()
    }

    #[test]
    fn round_trips_and_recompacts() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let mut log = DepsLog::load(path.clone()).unwrap();
        log.record("out.o", 42, &["a.h".to_owned(), "b.h".to_owned()])
            .unwrap();
        log.record("out.o", 43, &["a.h".to_owned()]).unwrap();
        let before = fs::metadata(&path).unwrap().len();

        let mut loaded = DepsLog::load(path.clone()).unwrap();
        assert_eq!(
            loaded.get("out.o"),
            Some(&DepsEntry {
                mtime: 43,
                inputs: vec!["a.h".to_owned()]
            })
        );
        loaded.recompact().unwrap();
        assert!(fs::metadata(&path).unwrap().len() < before);
        assert_eq!(DepsLog::load(path).unwrap().get("out.o").unwrap().mtime, 43);
    }

    #[test]
    fn preserves_output_node_order_through_recompaction() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let mut log = DepsLog::load(path.clone()).unwrap();
        log.record("z.o", 1, &["z.h".to_owned()]).unwrap();
        log.record("a.o", 2, &["a.h".to_owned()]).unwrap();
        assert_eq!(
            log.outputs_in_node_order().collect::<Vec<_>>(),
            ["z.o", "a.o"]
        );
        log.recompact().unwrap();
        assert_eq!(
            log.outputs_in_node_order().collect::<Vec<_>>(),
            ["z.o", "a.o"]
        );
    }

    #[test]
    fn recompacts_redundant_records_automatically() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let mut log = DepsLog::load(path.clone()).unwrap();
        for mtime in 0..=1_000 {
            log.record("out.o", mtime, &["input.h".to_owned()]).unwrap();
        }
        let before = fs::metadata(&path).unwrap().len();
        let loaded = DepsLog::load(path.clone()).unwrap();
        assert_eq!(loaded.get("out.o").unwrap().mtime, 1_000);
        assert!(fs::metadata(path).unwrap().len() < before);
    }

    #[test]
    fn malformed_log_is_discarded_before_new_records() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        fs::write(&path, b"not a deps log").unwrap();
        let mut log = DepsLog::load(path.clone()).unwrap();
        assert!(!path.exists());
        log.record("out.o", 9, &["input.h".to_owned()]).unwrap();
        assert_eq!(DepsLog::load(path).unwrap().get("out.o").unwrap().mtime, 9);
    }

    #[test]
    fn recovers_a_truncated_tail_before_appending() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let mut log = DepsLog::load(path.clone()).unwrap();
        log.record("out.o", 1, &["first.h".to_owned()]).unwrap();
        log.record("out2.o", 2, &["second.h".to_owned()]).unwrap();
        let length = fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length - 2)
            .unwrap();

        let mut recovered = DepsLog::load(path.clone()).unwrap();
        assert_eq!(recovered.get("out.o").unwrap().mtime, 1);
        assert!(recovered.get("out2.o").is_none());
        recovered
            .record("out2.o", 3, &["second.h".to_owned()])
            .unwrap();
        assert_eq!(DepsLog::load(path).unwrap().get("out2.o").unwrap().mtime, 3);
    }

    #[test]
    fn upstream_write_read_and_reverse_dependency_cases() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        write_sample_log(&path);
        let log = DepsLog::load(path).unwrap();
        assert_eq!(
            log.outputs_in_node_order().collect::<Vec<_>>(),
            ["out.o", "out2.o"]
        );
        assert_eq!(
            log.get("out.o"),
            Some(&DepsEntry {
                mtime: 1,
                inputs: vec!["foo.h".to_owned(), "bar.h".to_owned()],
            })
        );
        assert_eq!(
            log.get("out2.o"),
            Some(&DepsEntry {
                mtime: 2,
                inputs: vec!["foo.h".to_owned(), "bar2.h".to_owned()],
            })
        );
        assert_eq!(log.first_reverse_dep("foo.h"), Some("out.o"));
        assert_eq!(log.first_reverse_dep("bar.h"), Some("out.o"));
    }

    #[test]
    fn upstream_large_and_duplicate_dependency_entry_cases() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let inputs = (0..100_000)
            .map(|index| format!("file{index}.h"))
            .collect::<Vec<_>>();
        let mut log = DepsLog::load(path.clone()).unwrap();
        log.record("out.o", 1, &inputs).unwrap();
        assert_eq!(log.get("out.o").unwrap().inputs.len(), 100_000);
        drop(log);

        let mut loaded = DepsLog::load(path.clone()).unwrap();
        assert_eq!(loaded.get("out.o").unwrap().inputs.len(), 100_000);
        let before = fs::metadata(&path).unwrap().len();
        loaded.record("out.o", 1, &inputs).unwrap();
        assert_eq!(fs::metadata(path).unwrap().len(), before);
    }

    #[test]
    fn upstream_invalid_header_corpus() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        for contents in [
            &b""[..],
            &b"# ninjad"[..],
            &b"# ninjadeps\n"[..],
            &b"# ninjadeps\n\x01\x02"[..],
            &b"# ninjadeps\n\x01\x02\x03\x04"[..],
        ] {
            fs::write(&path, contents).unwrap();
            let log = DepsLog::load(path.clone()).unwrap();
            assert!(log.was_invalidated(), "contents={contents:?}");
            assert!(!path.exists(), "contents={contents:?}");
        }
    }

    #[test]
    fn upstream_truncation_corpus_recovers_monotonically() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let original = write_sample_log(&path);
        let mut prior_nodes = usize::MAX;
        let mut prior_entries = usize::MAX;
        for size in (1..=original.len()).rev() {
            fs::write(&path, &original[..size]).unwrap();
            let log = DepsLog::load(path.clone()).unwrap();
            if size < 16 {
                assert!(log.was_invalidated(), "size={size}");
                continue;
            }
            assert!(log.nodes.len() <= prior_nodes, "size={size}");
            assert!(log.entries.len() <= prior_entries, "size={size}");
            prior_nodes = log.nodes.len();
            prior_entries = log.entries.len();
        }
    }

    #[test]
    fn upstream_malformed_and_duplicate_path_records_recover_the_valid_prefix() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let original = write_sample_log(&path);

        for (name, bad) in [
            ("truncated size", original[..19].to_vec()),
            ("oversized record", {
                let mut bad = original.clone();
                bad[16..20].copy_from_slice(&0xffff_aa55u32.to_le_bytes());
                bad
            }),
            ("undersized record", {
                let mut bad = original.clone();
                bad[16..20].copy_from_slice(&1u32.to_le_bytes());
                bad
            }),
        ] {
            fs::write(&path, bad).unwrap();
            let log = DepsLog::load(path.clone()).unwrap();
            assert!(log.was_recovered(), "{name}");
            assert_eq!(fs::metadata(&path).unwrap().len(), 16, "{name}");
        }

        let mut duplicate = original.clone();
        duplicate.extend_from_slice(&[
            0x0c, 0x00, 0x00, 0x00, b'f', b'o', b'o', b'.', b'h', 0x00, 0x00, 0x00, 0xfe, 0xff,
            0xff, 0xff,
        ]);
        fs::write(&path, duplicate).unwrap();
        let recovered = DepsLog::load(path.clone()).unwrap();
        assert!(recovered.was_recovered());
        assert_eq!(recovered.get("out.o").unwrap().inputs.len(), 2);
        assert_eq!(fs::metadata(&path).unwrap().len(), original.len() as u64);
        drop(recovered);
        let clean = DepsLog::load(path).unwrap();
        assert!(!clean.was_recovered());
        assert_eq!(clean.get("out.o").unwrap().inputs.len(), 2);
    }

    #[test]
    fn upstream_recompact_removes_entries_without_live_deps_edges() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_deps");
        let mut log = DepsLog::load(path.clone()).unwrap();
        log.record("out.o", 1, &["foo.h".to_owned(), "bar.h".to_owned()])
            .unwrap();
        log.record("other_out.o", 1, &["foo.h".to_owned(), "baz.h".to_owned()])
            .unwrap();
        log.record("out.o", 1, &["foo.h".to_owned()]).unwrap();
        let before = fs::metadata(&path).unwrap().len();

        log.recompact_retain(|output| output == "out.o").unwrap();
        assert_eq!(log.get("out.o").unwrap().inputs, ["foo.h"]);
        assert!(log.get("other_out.o").is_none());
        assert!(fs::metadata(&path).unwrap().len() < before);

        log.recompact_retain(|_| false).unwrap();
        assert!(log.get("out.o").is_none());
        assert_eq!(fs::metadata(path).unwrap().len(), 16);
    }
}
