// use std::io::Read;

use symphonia_core::io::ReadBytes;

pub fn read_i8_be(r: &mut impl ReadBytes) -> i8 {
    let mut b = [0; 1];
    if let Err(e) = r.read_buf_exact(&mut b) {
        panic!("unable to read_i32_be {:?}", e)
    }
    i8::from_be_bytes(b)
}

pub fn read_i16_be(r: &mut impl ReadBytes) -> i16 {
    let mut b = [0; 2];
    if let Err(e) = r.read_buf_exact(&mut b) {
        panic!("unable to read_i32_be {:?}", e)
    }
    i16::from_be_bytes(b)
}

pub fn read_i32_be(r: &mut impl ReadBytes) -> i32 {
    let mut b = [0; 4];
    if let Err(e) = r.read_buf_exact(&mut b) {
        panic!("unable to read_i32_be {:?}", e)
    }
    i32::from_be_bytes(b)
}
