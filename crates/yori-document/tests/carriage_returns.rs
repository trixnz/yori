use yori_document::Document;

#[test]
fn bare_carriage_returns_load_without_changing_source_bytes() {
    for text in ["one\rtwo", "one\r\rtwo\r", "one\r\ntwo\rthree\nlast", "\r"] {
        let document = Document::from_bytes(text.as_bytes().to_vec()).unwrap();

        assert_eq!(document.text(), text);
        let reconstructed: String = (0..document.lines().len())
            .map(|line| document.full_line(line))
            .collect();
        assert_eq!(reconstructed, text);
    }
}

#[test]
fn repeated_carriage_returns_are_individual_line_endings() {
    let mut document = Document::from_bytes(b"one\r\rtwo\r".to_vec()).unwrap();

    assert_eq!(document.lines().len(), 3);
    assert_eq!(document.content(0), "one");
    assert_eq!(document.content(1), "");
    assert_eq!(document.content(2), "two");
    assert_eq!(document.newline(), "\r");
    assert_eq!(document.line_at_offset(5), 2);
    assert_eq!(document.line_at_offset(9), 3);

    document.replace(5..8, "changed").unwrap();

    assert_eq!(document.text(), "one\r\rchanged\r");
    assert_eq!(document.content(2), "changed");
}
