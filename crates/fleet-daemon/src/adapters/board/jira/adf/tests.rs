use super::render::document_nodes;
use super::*;

/// A description shaped like the ones the real backend pulls: headings, marked text, a
/// mention, an emoji, a link, nested lists, code, a panel, a table, a rule and an image.
const JIRA_DESCRIPTION: &str = r#"{
  "version": 1,
  "type": "doc",
  "content": [
    { "type": "heading", "attrs": { "level": 2 },
      "content": [{ "type": "text", "text": "Contexto" }] },
    { "type": "paragraph", "content": [
      { "type": "text", "text": "El " },
      { "type": "text", "text": "sync", "marks": [{ "type": "code" }] },
      { "type": "text", "text": " falla cuando el board tiene " },
      { "type": "text", "text": "más de 200", "marks": [{ "type": "strong" }] },
      { "type": "text", "text": " tarjetas. Reportado por " },
      { "type": "mention", "attrs": { "id": "557058:abc", "text": "@Danny Fuentes" } },
      { "type": "text", "text": " " },
      { "type": "emoji", "attrs": { "shortName": ":warning:", "text": "⚠️" } }
    ]},
    { "type": "paragraph", "content": [
      { "type": "text", "text": "Ver ", "marks": [] },
      { "type": "text", "text": "el runbook",
        "marks": [{ "type": "link", "attrs": { "href": "https://buk.dev/runbook" } }] },
      { "type": "text", "text": " y " },
      { "type": "inlineCard", "attrs": { "url": "https://buk.atlassian.net/browse/SP-1" } }
    ]},
    { "type": "heading", "attrs": { "level": 3 },
      "content": [{ "type": "text", "text": "Pasos" }] },
    { "type": "orderedList", "content": [
      { "type": "listItem", "content": [
        { "type": "paragraph", "content": [{ "type": "text", "text": "Abrir el board" }] } ] },
      { "type": "listItem", "content": [
        { "type": "paragraph", "content": [{ "type": "text", "text": "Sincronizar" }] },
        { "type": "bulletList", "content": [
          { "type": "listItem", "content": [
            { "type": "paragraph", "content": [
              { "type": "text", "text": "con " },
              { "type": "text", "text": "--full", "marks": [{ "type": "code" }] } ] } ] } ] } ] }
    ]},
    { "type": "codeBlock", "attrs": { "language": "bash" },
      "content": [{ "type": "text", "text": "fleet board sync --full\necho ok" }] },
    { "type": "panel", "attrs": { "panelType": "warning" }, "content": [
      { "type": "paragraph", "content": [
        { "type": "text", "text": "No correr en producción." } ] } ]},
    { "type": "rule" },
    { "type": "table", "attrs": { "isNumberColumnEnabled": false }, "content": [
      { "type": "tableRow", "content": [
        { "type": "tableHeader", "content": [
          { "type": "paragraph", "content": [{ "type": "text", "text": "Entorno" }] } ] },
        { "type": "tableHeader", "content": [
          { "type": "paragraph", "content": [{ "type": "text", "text": "Estado" }] } ] } ] },
      { "type": "tableRow", "content": [
        { "type": "tableCell", "content": [
          { "type": "paragraph", "content": [{ "type": "text", "text": "staging" }] } ] },
        { "type": "tableCell", "content": [
          { "type": "paragraph", "content": [
            { "type": "text", "text": "ok", "marks": [{ "type": "em" }] } ] } ] } ] }
    ]},
    { "type": "mediaSingle", "attrs": { "layout": "center" }, "content": [
      { "type": "media", "attrs": { "id": "9f", "type": "file", "collection": "c" } } ]},
    { "type": "paragraph", "content": [
      { "type": "text", "text": "línea uno" },
      { "type": "hardBreak" },
      { "type": "text", "text": "línea dos" }
    ]}
  ]
}"#;

fn doc(value: Value) -> Value {
    json!({ "version": 1, "type": "doc", "content": value })
}

fn paragraph(text: &str) -> Value {
    json!({ "type": "paragraph", "content": [{ "type": "text", "text": text }] })
}

fn fixture() -> Value {
    serde_json::from_str(JIRA_DESCRIPTION).expect("the fixture is valid JSON")
}

// -- ADF → markdown ------------------------------------------------------------------

/// The daemon feeds this whatever `acli` printed; a description that is `null` (Jira's
/// answer for "no description") must be the empty string, not a panic.
#[test]
fn a_missing_or_malformed_document_renders_as_nothing() {
    assert_eq!(adf_to_markdown(&Value::Null), "");
    assert_eq!(adf_to_markdown(&json!("just a string")), "");
    assert_eq!(adf_to_markdown(&json!(42)), "");
    assert_eq!(adf_to_markdown(&json!([1, 2, 3])), "");
    assert_eq!(adf_to_markdown(&json!({ "type": "doc" })), "");
    assert_eq!(
        adf_to_markdown(&json!({ "type": "doc", "content": "nonsense" })),
        ""
    );
    assert_eq!(adf_to_markdown(&doc(json!([]))), "");
}

/// Every field of every node can be the wrong type and the walk still terminates.
#[test]
fn nodes_with_wrong_typed_fields_never_panic() {
    let broken = doc(json!([
        { "type": 7 },
        { "type": "paragraph", "content": { "not": "an array" } },
        { "type": "heading", "attrs": "nope", "content": [{ "type": "text" }] },
        { "type": "heading", "attrs": { "level": "two" },
          "content": [{ "type": "text", "text": "still a heading" }] },
        { "type": "text", "text": 12, "marks": "no" },
        { "type": "codeBlock", "attrs": { "language": 3 } },
        { "type": "bulletList", "content": [{ "type": "listItem" }] },
        { "type": "table", "content": [{ "type": "tableRow" }] },
        { "type": "mention" },
        { "type": "emoji" },
        { "type": "inlineCard" },
        {}
    ]));
    // The heading with an unreadable level falls back to level 1, the empty code block to
    // a bare fence and the empty list item to a bare marker; nothing else survives.
    assert_eq!(
        adf_to_markdown(&broken),
        "# still a heading\n\n```\n\n```\n\n-"
    );
    assert_eq!(adf_plain_text(&broken), "still a heading");
}

#[test]
fn paragraphs_join_with_a_blank_line_and_empty_ones_drop_out() {
    let value = doc(json!([
        paragraph("one"),
        { "type": "paragraph", "content": [] },
        paragraph("two"),
    ]));
    assert_eq!(adf_to_markdown(&value), "one\n\ntwo");
}

#[test]
fn headings_render_one_hash_per_level() {
    for level in 1..=6u64 {
        let value = doc(json!([
            { "type": "heading", "attrs": { "level": level },
              "content": [{ "type": "text", "text": "H" }] }
        ]));
        assert_eq!(
            adf_to_markdown(&value),
            format!("{} H", "#".repeat(level as usize))
        );
    }
}

/// Jira only writes 1–6, but a level of 0 or 99 must not produce `H` with no hashes or a
/// hundred-hash line.
#[test]
fn an_out_of_range_heading_level_clamps() {
    let low = doc(json!([
        { "type": "heading", "attrs": { "level": 0 },
          "content": [{ "type": "text", "text": "H" }] }
    ]));
    let high = doc(json!([
        { "type": "heading", "attrs": { "level": 99 },
          "content": [{ "type": "text", "text": "H" }] }
    ]));
    assert_eq!(adf_to_markdown(&low), "# H");
    assert_eq!(adf_to_markdown(&high), "###### H");
}

#[test]
fn bullet_and_ordered_lists_render_their_markers() {
    let value = doc(json!([
        { "type": "bulletList", "content": [
            { "type": "listItem", "content": [paragraph("a")] },
            { "type": "listItem", "content": [paragraph("b")] } ]},
        { "type": "orderedList", "content": [
            { "type": "listItem", "content": [paragraph("x")] },
            { "type": "listItem", "content": [paragraph("y")] } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "- a\n- b\n\n1. x\n2. y");
}

/// `attrs.order` is how ADF says "this list starts at 3"; ignoring it renumbers the author.
#[test]
fn an_ordered_list_starts_at_its_order_attribute() {
    let value = doc(json!([
        { "type": "orderedList", "attrs": { "order": 3 }, "content": [
            { "type": "listItem", "content": [paragraph("a")] },
            { "type": "listItem", "content": [paragraph("b")] } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "3. a\n4. b");
}

#[test]
fn nested_lists_indent_under_their_parent_item() {
    let value = doc(json!([
        { "type": "orderedList", "content": [
            { "type": "listItem", "content": [
                paragraph("outer"),
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [
                        paragraph("inner"),
                        { "type": "bulletList", "content": [
                            { "type": "listItem", "content": [paragraph("deep")] } ]} ]} ]} ]} ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "1. outer\n   - inner\n     - deep");
}

#[test]
fn a_code_block_keeps_its_language_and_its_lines() {
    let with = doc(json!([
        { "type": "codeBlock", "attrs": { "language": "rust" },
          "content": [{ "type": "text", "text": "fn main() {}\nlet x = 1;" }] }
    ]));
    let without = doc(json!([
        { "type": "codeBlock", "content": [{ "type": "text", "text": "plain" }] }
    ]));
    assert_eq!(
        adf_to_markdown(&with),
        "```rust\nfn main() {}\nlet x = 1;\n```"
    );
    assert_eq!(adf_to_markdown(&without), "```\nplain\n```");
}

#[test]
fn a_blockquote_prefixes_every_line_including_the_blank_one() {
    let value = doc(json!([
        { "type": "blockquote", "content": [paragraph("first"), paragraph("second")] }
    ]));
    assert_eq!(adf_to_markdown(&value), "> first\n>\n> second");
}

#[test]
fn a_panel_renders_as_a_blockquote() {
    let value = doc(json!([
        { "type": "panel", "attrs": { "panelType": "info" }, "content": [paragraph("heads up")] }
    ]));
    assert_eq!(adf_to_markdown(&value), "> heads up");
}

#[test]
fn a_rule_renders_as_three_dashes() {
    let value = doc(json!([paragraph("a"), { "type": "rule" }, paragraph("b")]));
    assert_eq!(adf_to_markdown(&value), "a\n\n---\n\nb");
}

#[test]
fn a_hard_break_breaks_the_line_inside_the_paragraph() {
    let value = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "text", "text": "one" },
            { "type": "hardBreak" },
            { "type": "text", "text": "two" } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "one\ntwo");
}

#[test]
fn text_marks_render_as_their_markdown_syntax() {
    let mark = |name: &str| {
        doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "x", "marks": [{ "type": name }] } ]}
        ]))
    };
    assert_eq!(adf_to_markdown(&mark("strong")), "**x**");
    assert_eq!(adf_to_markdown(&mark("em")), "*x*");
    assert_eq!(adf_to_markdown(&mark("code")), "`x`");
    assert_eq!(adf_to_markdown(&mark("strike")), "~~x~~");
    assert_eq!(adf_to_markdown(&mark("underline")), "_x_");
    assert_eq!(adf_to_markdown(&mark("subsup")), "x");
}

#[test]
fn stacked_marks_nest_and_code_wins_over_emphasis() {
    let stacked = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "text", "text": "x",
              "marks": [{ "type": "strong" }, { "type": "em" }, { "type": "strike" }] } ]}
    ]));
    let coded = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "text", "text": "x",
              "marks": [{ "type": "code" }, { "type": "strong" }] } ]}
    ]));
    assert_eq!(adf_to_markdown(&stacked), "***~~x~~***");
    assert_eq!(adf_to_markdown(&coded), "`x`");
}

/// Jira stores the trailing space inside the strong run; `**bold **` is not bold anywhere.
#[test]
fn marks_keep_their_surrounding_whitespace_outside_the_syntax() {
    let value = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "text", "text": "a" },
            { "type": "text", "text": " bold ", "marks": [{ "type": "strong" }] },
            { "type": "text", "text": "b" } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "a **bold** b");
}

#[test]
fn a_link_renders_bare_when_its_text_is_its_href() {
    let bare = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "text", "text": "https://buk.dev",
              "marks": [{ "type": "link", "attrs": { "href": "https://buk.dev" } }] } ]}
    ]));
    let titled = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "text", "text": "docs",
              "marks": [{ "type": "link", "attrs": { "href": "https://buk.dev" } },
                        { "type": "strong" }] } ]}
    ]));
    assert_eq!(adf_to_markdown(&bare), "https://buk.dev");
    assert_eq!(adf_to_markdown(&titled), "[**docs**](https://buk.dev)");
}

#[test]
fn a_mention_renders_as_an_at_name_however_it_is_spelled() {
    let value = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "mention", "attrs": { "text": "@Ana Díaz" } },
            { "type": "text", "text": " y " },
            { "type": "mention", "attrs": { "displayName": "Beto" } },
            { "type": "text", "text": " y " },
            { "type": "mention", "attrs": { "id": "557058:x" } } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "@Ana Díaz y @Beto y ");
}

#[test]
fn an_emoji_renders_as_its_short_name_or_its_glyph() {
    let value = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "emoji", "attrs": { "shortName": ":tada:", "text": "🎉" } },
            { "type": "text", "text": " " },
            { "type": "emoji", "attrs": { "text": "🙂" } } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), ":tada: 🙂");
}

#[test]
fn inline_and_embed_cards_render_their_url() {
    let value = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "inlineCard", "attrs": { "url": "https://a.dev/1" } },
            { "type": "text", "text": " " },
            { "type": "embedCard", "attrs": { "data": { "url": "https://b.dev/2" } } } ]}
    ]));
    assert_eq!(adf_to_markdown(&value), "https://a.dev/1 https://b.dev/2");
}

/// Markdown cannot carry a Jira attachment, and a silent drop reads as a truncated body.
#[test]
fn media_renders_as_an_attachment_placeholder() {
    let value = doc(json!([
        { "type": "mediaSingle", "content": [{ "type": "media", "attrs": { "id": "1" } }] },
        { "type": "mediaGroup", "content": [
            { "type": "media", "attrs": { "id": "2" } },
            { "type": "media", "attrs": { "id": "3" } } ]},
        { "type": "paragraph", "content": [
            { "type": "text", "text": "ver " },
            { "type": "mediaInline", "attrs": { "id": "4" } } ]}
    ]));
    assert_eq!(
        adf_to_markdown(&value),
        "[attachment]\n\n[attachment]\n\n[attachment]\n\nver [attachment]"
    );
}

#[test]
fn a_table_renders_as_a_pipe_table_with_padded_rows() {
    let value = doc(json!([
        { "type": "table", "content": [
            { "type": "tableRow", "content": [
                { "type": "tableHeader", "content": [paragraph("a")] },
                { "type": "tableHeader", "content": [paragraph("b | c")] } ]},
            { "type": "tableRow", "content": [
                { "type": "tableCell", "content": [paragraph("1")] } ]}
        ]}
    ]));
    assert_eq!(
        adf_to_markdown(&value),
        "| a | b \\| c |\n| --- | --- |\n| 1 |  |"
    );
}

#[test]
fn an_unknown_node_keeps_its_content_and_a_childless_one_is_dropped() {
    let value = doc(json!([
        { "type": "expand", "attrs": { "title": "Detalles" },
          "content": [paragraph("inside"), { "type": "rule" }] },
        { "type": "status", "attrs": { "text": "DONE" } },
        paragraph("after"),
    ]));
    assert_eq!(adf_to_markdown(&value), "inside\n\n---\n\nafter");
}

/// A comment body arrives as a bare node often enough that rejecting one would be a bug.
#[test]
fn a_bare_block_node_renders_without_a_doc_wrapper() {
    assert_eq!(adf_to_markdown(&paragraph("hola")), "hola");
    assert_eq!(
        adf_to_markdown(&json!({ "content": [paragraph("hola")] })),
        "hola"
    );
}

// -- plain text ----------------------------------------------------------------------

#[test]
fn plain_text_flattens_every_block_to_one_line() {
    let value = doc(json!([
        { "type": "heading", "attrs": { "level": 1 },
          "content": [{ "type": "text", "text": "Title" }] },
        { "type": "paragraph", "content": [
            { "type": "text", "text": "bold", "marks": [{ "type": "strong" }] },
            { "type": "hardBreak" },
            { "type": "text", "text": "and  code", "marks": [{ "type": "code" }] } ]},
        { "type": "bulletList", "content": [
            { "type": "listItem", "content": [paragraph("a")] },
            { "type": "listItem", "content": [paragraph("b")] } ]}
    ]));
    assert_eq!(adf_plain_text(&value), "Title bold and code a b");
    assert_eq!(adf_plain_text(&Value::Null), "");
    assert_eq!(adf_plain_text(&json!({ "type": "doc" })), "");
}

/// A custom field holding an ADF value is rendered in a one-line property row, so the
/// mention, the emoji and the card must survive the flattening as text.
#[test]
fn plain_text_keeps_mentions_emoji_and_card_urls() {
    let value = doc(json!([
        { "type": "paragraph", "content": [
            { "type": "mention", "attrs": { "text": "@Ana" } },
            { "type": "text", "text": " " },
            { "type": "emoji", "attrs": { "shortName": ":tada:" } },
            { "type": "text", "text": " " },
            { "type": "inlineCard", "attrs": { "url": "https://a.dev" } } ]}
    ]));
    assert_eq!(adf_plain_text(&value), "@Ana :tada: https://a.dev");
}

// -- markdown → ADF ------------------------------------------------------------------

#[test]
fn an_empty_markdown_body_is_an_empty_document() {
    assert_eq!(markdown_to_adf(""), doc(json!([])));
    assert_eq!(markdown_to_adf("\n\n   \n"), doc(json!([])));
}

#[test]
fn markdown_headings_and_paragraphs_become_their_adf_nodes() {
    assert_eq!(
        markdown_to_adf("# One\n\nline a\nline b"),
        doc(json!([
            { "type": "heading", "attrs": { "level": 1 },
              "content": [{ "type": "text", "text": "One" }] },
            paragraph("line a line b"),
        ]))
    );
}

/// What the scanner still refuses: a hash with no space is not a heading, and a pipe row
/// with no `| --- |` under it is not a table. A bracket pair that is not a link — no
/// `](`, a href carrying whitespace, an empty half — stays the text it was written as.
#[test]
fn markdown_beyond_the_subset_stays_plain_text() {
    assert_eq!(
        markdown_to_adf("#nospace\n| a | b |\n[t] (u)"),
        doc(json!([paragraph("#nospace | a | b | [t] (u)")]))
    );
    for text in ["[t]()", "[](u)", r#"[t](u "title")"#, "[a[b]](u)"] {
        assert_eq!(
            markdown_to_adf(text),
            doc(json!([paragraph(text)])),
            "{text}"
        );
    }
}

/// Rule 2 on the one construct that carries a user's own words. `marked_text` writes a link
/// mark as `[label](href)`, and a scanner that could not read its own output turned every
/// labelled link of a pulled description into the literal characters `[`, `](` and `)`
/// around a link node whose text was the bare href — from an edit to another line entirely.
#[test]
fn a_labelled_link_reads_back_as_the_link_it_was_written_from() {
    let link = |text: &str, href: &str| {
        json!({ "type": "text", "text": text,
                "marks": [{ "type": "link", "attrs": { "href": href } }] })
    };
    assert_eq!(
        markdown_to_adf("see [the runbook](https://buk.dev/x) now"),
        doc(json!([{ "type": "paragraph", "content": [
            { "type": "text", "text": "see " },
            link("the runbook", "https://buk.dev/x"),
            { "type": "text", "text": " now" },
        ]}]))
    );
    // A href of its own parentheses is not ended by the first `)`, exactly as `trim_url_end`
    // refuses to cut one off a bare URL.
    assert_eq!(
        markdown_to_adf("[wiki](https://wiki/x_(y))"),
        doc(json!([{ "type": "paragraph",
                     "content": [link("wiki", "https://wiki/x_(y)")] }]))
    );
    // The label is markdown of its own: `marked_text` writes a node carrying both marks
    // this way, and both marks have to survive the trip back.
    assert_eq!(
        markdown_to_adf("[**bold**](u)"),
        doc(json!([{ "type": "paragraph", "content": [
            json!({ "type": "text", "text": "bold",
                    "marks": [{ "type": "strong" },
                              { "type": "link", "attrs": { "href": "u" } }] })
        ]}]))
    );
    // The full round trip, on the shape a pulled description actually holds.
    let source = "Ver [el runbook](https://buk.dev/runbook) y `sync`";
    assert_eq!(adf_to_markdown(&markdown_to_adf(source)), source);
}

/// The other half of rule 2: every block `adf_to_markdown` writes reads back as the node
/// it came from. Before this, one edited word rewrote a Jira table as a paragraph of
/// pipes, a rule as the text `---`, a panel as a paragraph starting `> `, and an h4 as the
/// literal `#### x`.
#[test]
fn markdown_reads_back_every_block_the_renderer_writes() {
    assert_eq!(
        markdown_to_adf("#### deep"),
        doc(json!([{ "type": "heading", "attrs": { "level": 4 },
                    "content": [{ "type": "text", "text": "deep" }] }]))
    );
    assert_eq!(markdown_to_adf("---"), doc(json!([{ "type": "rule" }])));
    assert_eq!(
        markdown_to_adf("> quoted\n> - a"),
        doc(json!([{
            "type": "blockquote",
            "content": [
                paragraph("quoted"),
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [paragraph("a")] }] },
            ],
        }]))
    );
    let cell =
        |kind: &str, text: &str| json!({ "type": kind, "attrs": {}, "content": [paragraph(text)] });
    assert_eq!(
        markdown_to_adf("| campo | valor |\n| --- | --- |\n| a | 1 |"),
        doc(json!([{
            "type": "table",
            "attrs": { "isNumberColumnEnabled": false, "layout": "default" },
            "content": [
                { "type": "tableRow", "content": [
                    cell("tableHeader", "campo"), cell("tableHeader", "valor")] },
                { "type": "tableRow", "content": [
                    cell("tableCell", "a"), cell("tableCell", "1")] },
            ],
        }]))
    );
    assert_eq!(
        markdown_to_adf("- uno\n  - uno.a"),
        doc(json!([{ "type": "bulletList", "content": [{
            "type": "listItem",
            "content": [
                paragraph("uno"),
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [paragraph("uno.a")] }] },
            ],
        }] }]))
    );
}

/// The whole point, end to end: a rich Jira description that a user edits one line of has
/// to come back as the same node types it went out as.
#[test]
fn a_rich_description_survives_an_edit_and_a_push_back() {
    let original = doc(json!([
        { "type": "heading", "attrs": { "level": 4 },
          "content": [{ "type": "text", "text": "Contexto" }] },
        paragraph("uno"),
        { "type": "bulletList", "content": [{
            "type": "listItem",
            "content": [
                paragraph("dos"),
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [paragraph("dos.a")] }] },
            ],
        }] },
        { "type": "rule" },
        { "type": "panel", "attrs": { "panelType": "info" },
          "content": [paragraph("ojo con esto")] },
        { "type": "table", "attrs": { "isNumberColumnEnabled": false, "layout": "default" },
          "content": [
            { "type": "tableRow", "content": [
                { "type": "tableHeader", "attrs": {}, "content": [paragraph("campo")] },
                { "type": "tableHeader", "attrs": {}, "content": [paragraph("valor")] }] },
            { "type": "tableRow", "content": [
                { "type": "tableCell", "attrs": {}, "content": [paragraph("a")] },
                { "type": "tableCell", "attrs": {}, "content": [paragraph("1")] }] },
          ] },
    ]));
    let edited = format!("{}\n\nuna línea más", adf_to_markdown(&original));
    let pushed = markdown_to_adf(&edited);
    let kinds: Vec<&str> = document_nodes(&pushed).iter().map(node_kind).collect();
    assert_eq!(
        kinds,
        vec![
            "heading",
            "paragraph",
            "bulletList",
            "rule",
            // A panel comes back as the blockquote it rendered as; its text is intact.
            "blockquote",
            "table",
            "paragraph",
        ]
    );
    // And the round trip is stable from there on.
    assert_eq!(adf_to_markdown(&pushed), edited);
}

#[test]
fn markdown_lists_become_adf_lists_and_do_not_merge() {
    let list = |kind: &str, items: Value| json!({ "type": kind, "content": items });
    let item = |text: &str| json!({ "type": "listItem", "content": [paragraph(text)] });
    assert_eq!(
        markdown_to_adf("- a\n* b\n1. c\n2. d"),
        doc(json!([
            list("bulletList", json!([item("a"), item("b")])),
            list("orderedList", json!([item("c"), item("d")])),
        ]))
    );
}

#[test]
fn an_ordered_list_that_does_not_start_at_one_carries_its_order() {
    let content = markdown_to_adf("3. a\n4. b");
    assert_eq!(content["content"][0]["attrs"], json!({ "order": 3 }));
    assert_eq!(markdown_to_adf("1. a")["content"][0].get("attrs"), None);
}

#[test]
fn markdown_fenced_code_carries_its_language_and_survives_an_open_fence() {
    assert_eq!(
        markdown_to_adf("```sh\nfleet board sync\n  --full\n```"),
        doc(json!([
            { "type": "codeBlock", "attrs": { "language": "sh" },
              "content": [{ "type": "text", "text": "fleet board sync\n  --full" }] }
        ]))
    );
    assert_eq!(
        markdown_to_adf("```\nleft open"),
        doc(json!([
            { "type": "codeBlock", "content": [{ "type": "text", "text": "left open" }] }
        ]))
    );
}

#[test]
fn markdown_inline_marks_and_urls_become_adf_marks() {
    assert_eq!(
        markdown_to_adf("run `make ci` then **ship** via https://fleet.dev/docs ok"),
        doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "run " },
                { "type": "text", "text": "make ci", "marks": [{ "type": "code" }] },
                { "type": "text", "text": " then " },
                { "type": "text", "text": "ship", "marks": [{ "type": "strong" }] },
                { "type": "text", "text": " via " },
                { "type": "text", "text": "https://fleet.dev/docs",
                  "marks": [{ "type": "link",
                              "attrs": { "href": "https://fleet.dev/docs" } }] },
                { "type": "text", "text": " ok" } ]}
        ]))
    );
}

/// A URL that ends in a parenthesis of its own keeps it; one the sentence put there does
/// not. Cutting the wrong one writes a link to a page that does not exist.
#[test]
fn a_url_keeps_the_parenthesis_it_opened_and_drops_the_sentence_s() {
    let href = |value: &Value| {
        value["content"][0]["content"][0]["marks"][0]["attrs"]["href"]
            .as_str()
            .map(str::to_owned)
    };
    assert_eq!(
        href(&markdown_to_adf("https://wiki.dev/x_(y)")).as_deref(),
        Some("https://wiki.dev/x_(y)")
    );
    assert_eq!(
        markdown_to_adf("(see https://wiki.dev/x)")["content"][0]["content"][1]["marks"][0]
            ["attrs"]["href"]
            .as_str(),
        Some("https://wiki.dev/x")
    );
    // Sentence punctuation after the URL's own parenthesis still goes.
    assert_eq!(
        href(&markdown_to_adf("https://wiki.dev/x_(y).")).as_deref(),
        Some("https://wiki.dev/x_(y)")
    );
}

/// The renderer leaves an unterminated `**` as two asterisks; Jira must show the same.
#[test]
fn unterminated_marks_stay_text_and_never_produce_an_empty_node() {
    assert_eq!(
        markdown_to_adf("**bold and `code"),
        doc(json!([paragraph("**bold and `code")]))
    );
    assert_eq!(
        markdown_to_adf("empty **** marks"),
        doc(json!([paragraph("empty **** marks")]))
    );
    // An empty inline-code span has no ADF spelling, so it drops out rather than becoming
    // the `{"type":"text","text":""}` Jira rejects.
    assert_eq!(
        markdown_to_adf("``"),
        doc(json!([{
            "type": "paragraph", "content": []
        }]))
    );
}

#[test]
fn multibyte_markdown_survives_span_splitting() {
    assert_eq!(
        markdown_to_adf("añ **ñu** fin"),
        doc(json!([
            { "type": "paragraph", "content": [
                { "type": "text", "text": "añ " },
                { "type": "text", "text": "ñu", "marks": [{ "type": "strong" }] },
                { "type": "text", "text": " fin" } ]}
        ]))
    );
}

// -- round trips ---------------------------------------------------------------------

/// The contract that matters: anything a user can type into a Fleet card description comes
/// back byte for byte after a trip through Jira.
#[test]
fn markdown_round_trips_through_adf_for_the_supported_subset() {
    for source in [
        "just a paragraph",
        "# One\n\n## Two\n\n### Three",
        "one paragraph\n\nanother paragraph",
        "- a\n- b",
        "1. a\n2. b",
        "3. a\n4. b",
        "- a\n\n1. b",
        "```rust\nfn main() {}\n```",
        "```\nno language\n```",
        "run `make ci` then **ship** via https://fleet.dev/docs ok",
        "see http://x.dev/a. and shttp://y.dev",
        "# Título\n\nañ **ñu** y `ñ`\n\n- ítem uno\n- ítem dos",
        "**all bold**",
        "a paragraph\n\n- a list\n- of two\n\n```sh\necho ok\n```\n\nand a tail",
    ] {
        let round_tripped = adf_to_markdown(&markdown_to_adf(source));
        assert_eq!(round_tripped, source, "round trip changed {source:?}");
    }
}

/// The other direction is not an identity — ADF is richer — but it must be stable: a body
/// pulled, rendered, edited and pushed twice must not drift.
#[test]
fn rendering_is_stable_across_a_second_trip() {
    let once = adf_to_markdown(&fixture());
    let twice = adf_to_markdown(&markdown_to_adf(&once));
    let thrice = adf_to_markdown(&markdown_to_adf(&twice));
    assert_eq!(twice, thrice);
    assert!(twice.contains("## Contexto"));
}

// -- the realistic fixture -----------------------------------------------------------

#[test]
fn a_realistic_jira_description_renders_as_readable_markdown() {
    assert_eq!(
        adf_to_markdown(&fixture()),
        concat!(
            "## Contexto\n",
            "\n",
            "El `sync` falla cuando el board tiene **más de 200** tarjetas. ",
            "Reportado por @Danny Fuentes :warning:\n",
            "\n",
            "Ver [el runbook](https://buk.dev/runbook) y ",
            "https://buk.atlassian.net/browse/SP-1\n",
            "\n",
            "### Pasos\n",
            "\n",
            "1. Abrir el board\n",
            "2. Sincronizar\n",
            "   - con `--full`\n",
            "\n",
            "```bash\n",
            "fleet board sync --full\n",
            "echo ok\n",
            "```\n",
            "\n",
            "> No correr en producción.\n",
            "\n",
            "---\n",
            "\n",
            "| Entorno | Estado |\n",
            "| --- | --- |\n",
            "| staging | *ok* |\n",
            "\n",
            "[attachment]\n",
            "\n",
            "línea uno\n",
            "línea dos",
        )
    );
}

#[test]
fn a_realistic_jira_description_flattens_to_one_line() {
    let plain = adf_plain_text(&fixture());
    assert!(!plain.contains('\n'));
    assert!(plain.starts_with("Contexto El sync falla cuando el board tiene más de 200"));
    assert!(plain.contains("@Danny Fuentes :warning:"));
    assert!(plain.contains("fleet board sync --full echo ok"));
    assert!(plain.contains("[attachment] línea uno línea dos"));
}

/// A Jira code block whose body contains a fence used to close early: the tail re-parsed as
/// loose paragraphs, and editing that description in Fleet wrote the mangled text back.
#[test]
fn a_code_block_containing_a_fence_round_trips_through_a_longer_one() {
    let adf = json!({
        "type": "doc",
        "version": 1,
        "content": [{
            "type": "codeBlock",
            "attrs": { "language": "md" },
            "content": [{ "type": "text", "text": "```sh\necho hi\n```" }]
        }]
    });
    let markdown = adf_to_markdown(&adf);
    assert!(
        markdown.starts_with("````md\n"),
        "the fence must outrun the body: {markdown}"
    );
    assert_eq!(
        markdown_to_adf(&markdown),
        adf,
        "and it has to come back whole"
    );
}
