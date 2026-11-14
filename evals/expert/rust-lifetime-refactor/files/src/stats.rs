use crate::record::Record;

/// Every key that appears in the records, sorted, each once.
pub fn distinct_keys(records: &[Record]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for record in records {
        for field in record.fields() {
            keys.push(field.key);
        }
    }
    keys.sort();
    keys.dedup();
    keys
}
