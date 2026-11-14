use std::collections::HashMap;

use crate::record::Record;

/// Groups records by the value of one key.
pub struct Index {
    groups: HashMap<String, Vec<Record>>,
}

impl Index {
    /// Builds the index. Records without the key are left out.
    pub fn build(records: &[Record], key: &str) -> Index {
        let mut groups: HashMap<String, Vec<Record>> = HashMap::new();
        for record in records {
            if let Some(value) = record.get(key) {
                groups.entry(value).or_default().push(record.clone());
            }
        }
        Index { groups }
    }

    /// The records whose value for the key equals `value`, in input order.
    pub fn get(&self, value: &str) -> Vec<Record> {
        self.groups.get(value).cloned().unwrap_or_default()
    }

    /// The distinct values of the key, sorted.
    pub fn values(&self) -> Vec<String> {
        let mut values: Vec<String> = self.groups.keys().cloned().collect();
        values.sort();
        values
    }
}
