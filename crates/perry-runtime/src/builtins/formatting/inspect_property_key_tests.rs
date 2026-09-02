use super::format_inspect_property_key;

#[test]
fn keeps_node_style_ascii_identifiers_bare() {
    assert_eq!(format_inspect_property_key("alpha_2"), "alpha_2");
    assert_eq!(format_inspect_property_key("_header"), "_header");
}

#[test]
fn quotes_non_identifier_property_names() {
    assert_eq!(
        format_inspect_property_key("Transfer-Encoding"),
        "'Transfer-Encoding'"
    );
    assert_eq!(format_inspect_property_key("1"), "'1'");
    assert_eq!(format_inspect_property_key("$value"), "'$value'");
    assert_eq!(format_inspect_property_key("x'y"), "\"x'y\"");
    assert_eq!(format_inspect_property_key("line\nbreak"), "'line\\nbreak'");
}
