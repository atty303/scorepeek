use crate::{Input, Output};
use std::cell::RefCell;

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[must_use]
pub fn allocate(length: i32) -> i32 {
    if length <= 0 {
        return 0;
    }
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    INPUT.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        buffer.resize(length, 0);
        buffer.as_mut_ptr() as i32
    })
}

/// # Errors
/// Returns an error when the buffer is not the current allocation or JSON is invalid.
pub fn decode(pointer: i32, length: i32) -> Result<Input, String> {
    let length = usize::try_from(length).map_err(|_| "input length is negative")?;
    INPUT.with(|buffer| {
        let buffer = buffer.borrow();
        if buffer.as_ptr() as i32 != pointer || buffer.len() != length {
            return Err("input does not match the allocated ABI buffer".into());
        }
        serde_json::from_slice(&buffer).map_err(|error| error.to_string())
    })
}

pub fn deallocate(pointer: i32, length: i32) {
    let Ok(length) = usize::try_from(length) else {
        return;
    };
    INPUT.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        if buffer.as_ptr() as i32 == pointer && buffer.len() == length {
            buffer.clear();
        }
    });
}

#[must_use]
pub fn encode(output: &Output) -> i64 {
    OUTPUT.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        *buffer = serde_json::to_vec(output).unwrap_or_else(|_| br#"{"schedule":{"kind":"idle"},"tree":{"kind":"element","key":"error","tag":"main","attributes":{},"children":[]}}"#.to_vec());
        let pointer = u64::try_from(buffer.as_ptr() as usize).unwrap_or_default() & 0xffff_ffff;
        let length = u64::try_from(buffer.len()).unwrap_or_default() & 0xffff_ffff;
        ((pointer << 32) | length).cast_signed()
    })
}
