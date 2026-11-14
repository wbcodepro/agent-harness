/// One `key=value` pair of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub value: String,
}

/// One parsed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    line: usize,
    raw: String,
    fields: Vec<Field>,
}

impl Record {
    pub fn new(line: usize, raw: String, fields: Vec<Field>) -> Record {
        Record { line, raw, fields }
    }

    /// The 1-based number of the line in the input.
    pub fn line(&self) -> usize {
        self.line
    }

    /// The text of the whole line, without the newline.
    pub fn raw(&self) -> String {
        self.raw.clone()
    }

    /// The fields in the order they appear on the line.
    pub fn fields(&self) -> Vec<Field> {
        self.fields.clone()
    }

    /// The value of the first field with this key.
    pub fn get(&self, key: &str) -> Option<String> {
        self.fields.iter().find(|f| f.key == key).map(|f| f.value.clone())
    }
}
