use agentd_core::dot::parser;
use agentd_core::graph::NodeGraph;

fn main() {
    // D: hyphen-starting attr value or ident — read_ident allows '-' as continuation
    // but a token STARTING with '-' is treated as arrow. What about "a-b" as ident?
    probe(
        "D: ident with embedded hyphen",
        r#"digraph m { "x" [k=a-b]; }"#,
    );

    // E: a single '-' not followed by '>' (e.g. negative number value)
    probe(
        "E: attr value -5 (leading minus)",
        r#"digraph m { "x" [k=-5]; }"#,
    );

    // F: empty pre_tools token via leading comma -> tool_name on ""
    probe(
        "F: pre_tools with empty token ',,'",
        r#"digraph m {
        "start" [shape=Mdiamond];
        "w" [handler=tool, pre_tools=",check_inbox,"];
        "end" [shape=Msquare];
        "start" -> "w"; "w" -> "end";
    }"#,
    );

    // G: comment '//' with NO trailing content then EOF inside graph
    probe(
        "G: trailing // comment to EOF",
        "digraph m { \"a\" [shape=Mdiamond]; \"b\" [shape=Msquare]; \"a\"->\"b\" // trailing",
    );

    // H: unbalanced parens in pre_tools -> split_top_level_commas depth underflow
    probe(
        "H: pre_tools unbalanced parens",
        r#"digraph m {
        "start" [shape=Mdiamond];
        "w" [handler=tool, pre_tools="check_inbox),mempal_search"];
        "end" [shape=Msquare];
        "start" -> "w"; "w" -> "end";
    }"#,
    );

    // I: subgraph keyword detection is via Ident match — what about "subgraph" as a node id quoted?
    probe(
        "I: quoted node literally named subgraph",
        r#"digraph m {
        "start" [shape=Mdiamond];
        "subgraph" [handler=tool];
        "end" [shape=Msquare];
        "start" -> "subgraph"; "subgraph" -> "end";
    }"#,
    );

    // J: attribute key/value where value is keyword 'digraph' (no special handling expected)
    probe(
        "J: bare value with dot e.g. handler=wait.human",
        r#"digraph m {
        "start" [shape=Mdiamond];
        "w" [handler=wait.human];
        "end" [shape=Msquare];
        "start" -> "w"; "w" -> "end";
    }"#,
    );
}

fn probe(label: &str, src: &str) {
    print!("=== {label} === ");
    match parser::parse(src) {
        Ok(ast) => {
            print!("PARSE OK; ");
            match NodeGraph::from_ast(&ast) {
                Ok(_) => println!("VALIDATE OK"),
                Err(e) => println!("VALIDATE ERR: {e}"),
            }
        }
        Err(e) => println!("PARSE ERR: {e}"),
    }
}
