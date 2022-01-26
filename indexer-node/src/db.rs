use rocksdb::{DBWithThreadMode, Direction, Error, IteratorMode, MultiThreaded};

use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Handle {
    pub db: Arc<DBWithThreadMode<MultiThreaded>>,
}

impl Handle {
    /// Creates a DB handle and stores its data at the path (folder).
    pub fn from_path(path: &str) -> Result<Handle, Error> {
        type Mdb = DBWithThreadMode<MultiThreaded>;
        let db = Mdb::open_default(path)?;
        //let db = DB::open_default(path)?;
        Ok(Handle { db: Arc::new(db) })
    }

    /// Key should hold alias::urn, and value holds the server
    /// in which repo is hosted.
    pub fn add_repository<K, V>(&self, k: K, v: V) -> Result<(), Error>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let k = [b"repo::", k.as_ref()].concat();
        self.db.put(k, v)
    }

    /// Iterates through all keys starting with prefix and returns (key, value).
    fn iterate_prefix<P>(&self, prefix: P) -> impl Iterator<Item = (String, String)> + '_
    where
        P: AsRef<[u8]> + 'static,
    {
        self.db
            .iterator(IteratorMode::From(prefix.as_ref(), Direction::Forward))
            .into_iter()
            .take_while(move |(k, _)| k.starts_with(prefix.as_ref()))
            // This is safe because inputs are checked on insertion
            // and leaking the Box leaves sole ownership to String.
            .map(|(k, v)| unsafe {
                let k = Box::leak(k);
                let v = Box::leak(v);
                (
                    String::from_raw_parts(k.as_mut_ptr(), k.len(), k.len()),
                    String::from_raw_parts(v.as_mut_ptr(), v.len(), v.len()),
                )
            })
    }

    /// Iterates through all keys starting *from* prefix and returns (key, value).
    pub fn iterate_from_prefix<P>(&self, prefix: P) -> impl Iterator<Item = (String, String)> + '_
    where
        P: AsRef<[u8]> + 'static,
    {
        self.db
            .iterator(IteratorMode::From(prefix.as_ref(), Direction::Forward))
            .into_iter()
            // This is safe because inputs are checked on insertion
            // and leaking the Box leaves sole ownership to String.
            .map(|(k, v)| unsafe {
                let k = Box::leak(k);
                let v = Box::leak(v);
                (
                    String::from_raw_parts(k.as_mut_ptr(), k.len(), k.len()),
                    String::from_raw_parts(v.as_mut_ptr(), v.len(), v.len()),
                )
            })
    }

    /// Lists all repositories' alias::urn (key) and servers (value) hosting them.
    pub fn list_repositories(&self) -> impl Iterator<Item = (String, String)> + '_ {
        let prefix = b"repo::";
        self.iterate_prefix(prefix)
    }

    /*
    /// Lists all repositories starting with alias::<any_urn>.
    pub fn repos_starting_with(&self, alias: &str) -> impl Iterator<Item = (String, String)> + '_ {
        let prefix = [b"repo::", alias.as_bytes()].concat();
        self.iterate_prefix(prefix)
    }

    /// Lists all users starting with username::<any_urn>.
    pub fn users_starting_with(
        &self,
        username: &str,
    ) -> impl Iterator<Item = (String, String)> + '_ {
        let prefix = [b"user::", username.as_bytes()].concat();
        self.iterate_prefix(prefix)
    }

    /// Key should hold username::urn, and value holds the server
    /// in which user is active.
    pub fn add_user<K, V>(&self, k: K, v: V) -> Result<(), Error>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let k = [b"user::", k.as_ref()].concat();
        self.db.put(k, v)
    }
    */
}
