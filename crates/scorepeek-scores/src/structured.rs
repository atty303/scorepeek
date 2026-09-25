//! Relational scalar representation of a validated RESULT. Evidence JSON is never read here.

use rusqlite::{Connection, Transaction, params};
use serde_json::{Map, Number, Value};

use crate::Error;

pub(super) const SCHEMA: &str = "CREATE TABLE result_attributes (
    event_id TEXT NOT NULL,
    path TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('object','array','string','integer','boolean','null')),
    text_value TEXT,
    integer_value INTEGER,
    PRIMARY KEY(event_id,path),
    FOREIGN KEY(event_id) REFERENCES play_results(event_id) ON DELETE CASCADE
);";

pub(super) fn save(tx: &Transaction<'_>, event_id: &str, result: &Value) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM result_attributes WHERE event_id=?1",
        [event_id],
    )?;
    write(tx, event_id, "", result)
}

fn write(tx: &Transaction<'_>, event_id: &str, path: &str, value: &Value) -> Result<(), Error> {
    let (kind, text, integer) = match value {
        Value::Object(_) => ("object", None, None),
        Value::Array(_) => ("array", None, None),
        Value::String(value) => ("string", Some(value.as_str()), None),
        Value::Number(value) => ("integer", None, value.as_i64()),
        Value::Bool(value) => ("boolean", None, Some(i64::from(*value))),
        Value::Null => ("null", None, None),
    };
    if matches!(value, Value::Number(_)) && integer.is_none() {
        return Err(Error::UnsupportedContract);
    }
    tx.execute(
        "INSERT INTO result_attributes(event_id,path,kind,text_value,integer_value) VALUES (?1,?2,?3,?4,?5)",
        params![event_id, path, kind, text, integer],
    )?;
    match value {
        Value::Object(fields) => {
            for (name, child) in fields {
                write(tx, event_id, &format!("{path}/{}", escape(name)), child)?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                write(tx, event_id, &format!("{path}/{index}"), child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn escape(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

pub(super) fn load(connection: &Connection, event_id: &str) -> Result<Option<Value>, Error> {
    let mut statement = connection.prepare(
        "SELECT path,kind,text_value,integer_value FROM result_attributes WHERE event_id=?1 ORDER BY length(path),path",
    )?;
    let rows = statement.query_map([event_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut root = None;
    for row in rows {
        let (path, kind, text, integer) = row?;
        let value = match kind.as_str() {
            "object" => Value::Object(Map::new()),
            "array" => Value::Array(Vec::new()),
            "string" => Value::String(text.ok_or(Error::UnsupportedContract)?),
            "integer" => Value::Number(Number::from(integer.ok_or(Error::UnsupportedContract)?)),
            "boolean" => Value::Bool(integer.ok_or(Error::UnsupportedContract)? != 0),
            "null" => Value::Null,
            _ => return Err(Error::UnsupportedContract),
        };
        if path.is_empty() {
            root = Some(value);
        } else {
            let target = root.as_mut().ok_or(Error::UnsupportedContract)?;
            let (parent, name) = path.rsplit_once('/').ok_or(Error::UnsupportedContract)?;
            let parent = if parent.is_empty() {
                target
            } else {
                target
                    .pointer_mut(parent)
                    .ok_or(Error::UnsupportedContract)?
            };
            match parent {
                Value::Object(map) => {
                    map.insert(name.replace("~1", "/").replace("~0", "~"), value);
                }
                Value::Array(array) => {
                    let index: usize = name.parse().map_err(|_| Error::UnsupportedContract)?;
                    if index != array.len() {
                        return Err(Error::UnsupportedContract);
                    }
                    array.push(value);
                }
                _ => return Err(Error::UnsupportedContract),
            }
        }
    }
    Ok(root)
}
