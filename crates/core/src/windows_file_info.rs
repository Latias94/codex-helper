use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use winapi_util::file::{Information, information};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
};

pub(crate) fn file_information(file: &File) -> io::Result<Information> {
    information(file)
}

pub(crate) fn path_information_no_follow(path: &Path) -> io::Result<Information> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    file_information(&file)
}

pub(crate) fn is_reparse_point(information: &Information) -> bool {
    information.file_attributes() & u64::from(FILE_ATTRIBUTE_REPARSE_POINT) != 0
}
