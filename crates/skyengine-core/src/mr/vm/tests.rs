use super::stdlib::*;
use super::*;

#[test]
fn table_insert_shifts_sequence_values() {
    let table = Table::new();
    table.borrow_mut().set(Value::Number(1.0), bytes(b"a"));
    table.borrow_mut().set(Value::Number(2.0), bytes(b"c"));
    table_insert(&[Value::Table(table.clone()), Value::Number(2.0), bytes(b"b")]).unwrap();
    assert_eq!(table.borrow().sequence_len(), 3);
    assert!(
        table
            .borrow()
            .get(&Value::Number(2.0))
            .raw_equal(&bytes(b"b"))
    );
}

#[test]
fn native_number_supports_radix() {
    assert!(native_tonumber(&[bytes(b"7f"), Value::Number(16.0)]).raw_equal(&Value::Number(127.0)));
    assert!(matches!(
        native_tonumber(&[bytes(b"not-a-number")]),
        Value::Nil
    ));
}

#[test]
fn c_string_helpers_stop_at_nul() {
    let value = bytes(b"abc\0def");
    assert_eq!(
        string_clen(std::slice::from_ref(&value)).unwrap()[0].number(),
        Some(3.0)
    );
    assert!(string_cstr(&[value]).unwrap()[0].raw_equal(&bytes(b"abc")));
}

#[test]
fn sub_value_splits_numbers_and_byte_strings_into_little_endian_words() {
    let number_bits = 0x1122_3344_aabb_ccdd_u64;
    let number = string_sub_value(&[Value::Number(f64::from_bits(number_bits))]).unwrap();
    assert_eq!(number[0].number(), Some(0xaabb_ccdd_u32 as f64));
    assert_eq!(number[1].number(), Some(0x1122_3344_u32 as f64));

    let string = string_sub_value(&[bytes(b"unknow")]).unwrap();
    assert_eq!(string[0].number(), Some(0x6e6b_6e75_u32 as f64));
    assert_eq!(string[1].number(), Some(0x0000_776f_u32 as f64));
}

#[test]
fn work_paths_map_the_guest_c_drive_to_the_runtime_root() {
    let root = std::path::Path::new("device");
    assert_eq!(
        safe_work_path(root, b"C:/mythroad/system/font.uc2"),
        Some(PathBuf::from("device/mythroad/system/font.uc2"))
    );
    assert_eq!(
        safe_work_path(root, b"c:\\mythroad\\system\\font.uc2"),
        Some(PathBuf::from("device/mythroad/system/font.uc2"))
    );
    assert_eq!(safe_work_path(root, b"D:/mythroad/file"), None);
    assert_eq!(safe_work_path(root, b"C:/../file"), None);
}
