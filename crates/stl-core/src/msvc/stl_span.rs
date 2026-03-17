use std::ptr::NonNull;
use std::ops::Deref;

use crate::types as c;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct span<T> {
    data: *const T,
    size: c::size_t
}

impl<T> span<T> {
    pub fn new(data: *const T, size: c::size_t) -> Self {
        Self {
            data, size
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn data_nonnull(&self) -> *const T {
        if self.data.is_null() {
            NonNull::dangling().as_ptr()
        } else {
            self.data
        }
    }
}

impl<T> From<&[T]> for span<T> {
    fn from(value: &[T]) -> Self {
        Self::new(
            value.as_ptr(),
            value.len()
        )
    }
}

impl<T> Deref for span<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.data_nonnull(), self.size()) }
    }
}