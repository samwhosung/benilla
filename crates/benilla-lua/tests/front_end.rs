//! The front end over every Lua chunk in the addon corpus and the install's interface files: each
//! compiles, but for the corpus chunks the script host's C Lua build rejected, which must fail
//! with the message it gave.
//!
//! Each compiled chunk's shape (constants in order, register count, parameters, locals,
//! upvalues, line runs) must also be the one `front_end.digest` records: a line per chunk, a hash
//! of its source and a hash of its shape. The shapes are the C build's, but where benilla-lua
//! since follows the client's 5.0 against 5.1 (the `for` loops' locals, closed once at the
//! loop's end): such a change rewrites the digest with `BENILLA_BLESS=1`,
//! which prints each chunk whose shape moved, for the review. A chunk whose source is not in it
//! (another corpus version) is not checked against it.

use std::path::{Path, PathBuf};

struct Chunk {
    name: String,
    bytes: Vec<u8>,
}

/// `benilla_ui::source::chunk`: one leading UTF-8 BOM and a leading `#` line go.
fn prepare(bytes: &[u8]) -> Vec<u8> {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    match b.first() {
        Some(b'#') => match b.iter().position(|&c| c == b'\n') {
            Some(nl) => b[nl..].to_vec(),
            None => Vec::new(),
        },
        _ => b.to_vec(),
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Inline `<Script>` text and every handler body in an XML file, each as its own chunk.
fn xml_chunks(name: &str, bytes: &[u8], out: &mut Vec<Chunk>) {
    let text =
        String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes))
            .into_owned();
    let Ok(doc) = roxmltree::Document::parse(&text) else {
        return;
    };
    for n in doc.descendants().filter(|n| n.is_element()) {
        let tag = n.tag_name().name();
        let inside_scripts = n.parent().is_some_and(|p| p.tag_name().name() == "Scripts");
        if (tag == "Script" && n.attribute("file").is_none()) || inside_scripts {
            let body: String = n.children().filter_map(|c| c.text()).collect();
            if !body.trim().is_empty() {
                out.push(Chunk {
                    name: format!("{name}:{tag}"),
                    bytes: body.into_bytes(),
                });
            }
        }
    }
}

fn corpus_chunks(out: &mut Vec<Chunk>) -> bool {
    let Some(root) = benilla_formats::addon_corpus() else {
        return false;
    };
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    for f in files {
        let name = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let ext = f
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let Ok(bytes) = std::fs::read(&f) else {
            continue;
        };
        match ext.as_deref() {
            Some("lua") => out.push(Chunk {
                name,
                bytes: prepare(&bytes),
            }),
            Some("xml") => xml_chunks(&name, &bytes, out),
            _ => {}
        }
    }
    true
}

fn chain_chunks(out: &mut Vec<Chunk>) -> bool {
    let Some(data) = benilla_formats::wow_data() else {
        return false;
    };
    let chain = benilla_formats::open_chain(&data).expect("open the patch chain");
    let mut names: Vec<String> = chain
        .list()
        .expect("list the chain")
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.to_ascii_lowercase().starts_with("interface\\"))
        .collect();
    names.sort();
    names.dedup();
    for name in names {
        let lower = name.to_ascii_lowercase();
        let Ok(bytes) = chain.read(&name) else {
            continue;
        };
        if lower.ends_with(".lua") {
            out.push(Chunk {
                name,
                bytes: prepare(&bytes),
            });
        } else if lower.ends_with(".xml") {
            xml_chunks(&name, &bytes, out);
        }
    }
    true
}

/// The chunks the C Lua build rejected, with its messages: 5.1 constructs the 1.12 grammar lacks,
/// and handlers written as prose.
const REJECTED: &[(&str, &str)] = &[
    ("AceGUI/AceGUIDropDownMenu.lua", "AceGUI/AceGUIDropDownMenu.lua:14: `<name>' expected near `<eof>'"),
    ("AckisRecipeList/ARLDataminer.lua", "AckisRecipeList/ARLDataminer.lua:2: unexpected symbol near `#'"),
    ("AckisRecipeList/ARLFrame.lua", "AckisRecipeList/ARLFrame.lua:1043: unexpected symbol near `#'"),
    ("AckisRecipeList/AckisRecipeList.lua", "AckisRecipeList/AckisRecipeList.lua:1604: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceAddon-3.0/AceAddon-3.0.lua", "AckisRecipeList/libs/AceAddon-3.0/AceAddon-3.0.lua:63: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceConfig-3.0/AceConfigCmd-3.0/AceConfigCmd-3.0.lua", "AckisRecipeList/libs/AceConfig-3.0/AceConfigCmd-3.0/AceConfigCmd-3.0.lua:43: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceConfig-3.0/AceConfigDialog-3.0/AceConfigDialog-3.0.lua", "...ecipeList/libs/AceConfig-3.0/AceConfigDialog-3.0/AceConfigDialog-3.0.lua:76: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceConfig-3.0/AceConfigRegistry-3.0/AceConfigRegistry-3.0.lua", "...eList/libs/AceConfig-3.0/AceConfigRegistry-3.0/AceConfigRegistry-3.0.lua:43: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceConsole-3.0/AceConsole-3.0.lua", "AckisRecipeList/libs/AceConsole-3.0/AceConsole-3.0.lua:31: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceEvent-3.0/AceEvent-3.0.lua", "AckisRecipeList/libs/AceEvent-3.0/AceEvent-3.0.lua:70: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceGUI-3.0/AceGUI-3.0.lua", "AckisRecipeList/libs/AceGUI-3.0/AceGUI-3.0.lua:72: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-DropDown-Items.lua", "AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-DropDown-Items.lua:7: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-DropDown.lua", "AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-DropDown.lua:8: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-Icon.lua", "AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-Icon.lua:30: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-Label.lua", "AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-Label.lua:83: unexpected symbol near `...'"),
    ("AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-TabGroup.lua", "AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-TabGroup.lua:173: unexpected symbol near `#'"),
    ("AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-TreeGroup.lua", "AckisRecipeList/libs/AceGUI-3.0/widgets/AceGUIWidget-TreeGroup.lua:316: unexpected symbol near `#'"),
    ("AckisRecipeList/libs/CallbackHandler-1.0/CallbackHandler-1.0.lua", "AckisRecipeList/libs/CallbackHandler-1.0/CallbackHandler-1.0.lua:91: unexpected symbol near `...'"),
    ("Enchantrix/EnchantrixBarker.xml:OnValueChanged", "Enchantrix/EnchantrixBarker.xml:OnValueChanged:1: `=' expected near `need'"),
    ("FixCTGroups/FixCTGroups.xml:OnLoad", "FixCTGroups/FixCTGroups.xml:OnLoad:2: `=' expected near `pressing'"),
    ("FixCTGroups/FixCTGroupsCells.xml:OnLoad", "FixCTGroups/FixCTGroupsCells.xml:OnLoad:2: `=' expected near `pressing'"),
    ("FuBar_AtlasFu/Libs/Tablet-2.0/Tablet-2.0.lua", "FuBar_AtlasFu/Libs/Tablet-2.0/Tablet-2.0.lua:150: unexpected symbol near `#'"),
    ("TipBuddy/TipBuddy.xml:OnEnter", "TipBuddy/TipBuddy.xml:OnEnter:3: `=' expected near `TipBuddy'"),
    ("TipBuddy/TipBuddy.xml:OnUpdate", "TipBuddy/TipBuddy.xml:OnUpdate:3: `=' expected near `should'"),
];

/// One function's shape, the parts of a 5.1 `string.dump` the code generator must match. Its
/// `Debug` text is what `front_end.digest` hashes, so a field's type and order stay as the C
/// build's undump wrote them.
#[derive(Debug, PartialEq)]
struct Shape {
    linedefined: u32,
    nups: u8,
    numparams: u8,
    vararg: bool,
    maxstack: u8,
    /// Each instruction's line, runs of one line taken as one: 5.1 spends two instructions on
    /// a test benilla fuses into one.
    lines: Vec<u32>,
    consts: Vec<String>,
    locals: Vec<String>,
    upvals: Vec<String>,
    protos: Vec<Shape>,
}

fn shape(p: &benilla_lua::proto::Proto, heap: &benilla_lua::heap::Heap) -> Shape {
    use benilla_lua::value::V;
    Shape {
        linedefined: p.line_defined,
        nups: p.upvals.len() as u8,
        numparams: p.nparams,
        vararg: p.vararg,
        // The client's `maxstacksize`, without benilla's own registers.
        maxstack: p.c_maxstack,
        lines: {
            let mut l = p.lines.clone();
            l.dedup();
            l
        },
        consts: p
            .consts
            .iter()
            .map(|v| match v.unpack() {
                V::Nil => "nil".to_string(),
                V::Bool(b) => format!("{b}"),
                V::Num(n) => format!("{n:?}"),
                V::Str(s) => format!("{:?}", String::from_utf8_lossy(heap.str(s))),
                v => panic!("constant {v:?}"),
            })
            .collect(),
        locals: p
            .locvars
            .iter()
            .map(|l| String::from_utf8_lossy(&l.name).into_owned())
            .collect(),
        upvals: p
            .upval_names
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect(),
        protos: p.protos.iter().map(|q| shape(q, heap)).collect(),
    }
}

/// FNV-1a, 64 bits: stable across platforms and Rust versions, which `DefaultHasher` is not.
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// `front_end.digest`: each source hash's shape hash, as the C build compiled it.
fn read_digest() -> std::collections::HashMap<u64, u64> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/front_end.digest");
    std::fs::read_to_string(path)
        .expect("the digest file")
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(a, b)| {
            (
                u64::from_str_radix(a, 16).unwrap(),
                u64::from_str_radix(b, 16).unwrap(),
            )
        })
        .collect()
}

fn write_digest(d: &std::collections::HashMap<u64, u64>) {
    let mut lines: Vec<String> = d
        .iter()
        .map(|(a, b)| format!("{a:016x} {b:016x}"))
        .collect();
    lines.sort();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/front_end.digest");
    std::fs::write(path, lines.join("\n") + "\n").expect("write the digest file");
}

#[test]
fn every_chunk_compiles_as_the_c_build_compiled_it() {
    let mut chunks = Vec::new();
    let corpus = corpus_chunks(&mut chunks);
    let chain = chain_chunks(&mut chunks);
    if !corpus && !chain {
        benilla_formats::skipped("no addon corpus and no WoW install", &[]);
        return;
    }
    let got: Vec<(&str, String)> = chunks
        .iter()
        .filter_map(|c| {
            let chunkname = format!("@{}", c.name);
            let e = benilla_lua::check(&c.bytes, chunkname.as_bytes()).err()?;
            Some((c.name.as_str(), e.to_string()))
        })
        .collect();
    // The rejected chunks are all the corpus's.
    let want: Vec<(&str, String)> = REJECTED
        .iter()
        .filter(|_| corpus)
        .map(|&(n, m)| (n, m.to_string()))
        .collect();
    println!("{} chunks, {} rejected", chunks.len(), got.len());
    assert_eq!(got, want);

    let mut recorded = read_digest();
    let bless = std::env::var_os("BENILLA_BLESS").is_some_and(|v| v == "1");
    let mut heap = benilla_lua::heap::Heap::default();
    let (mut checked, mut differ) = (0, Vec::new());
    for c in &chunks {
        let key = fnv64(&c.bytes);
        let Some(&want) = recorded.get(&key) else {
            continue;
        };
        let chunkname = format!("@{}", c.name);
        let p = benilla_lua::compile(&c.bytes, chunkname.as_bytes(), &mut heap).unwrap();
        checked += 1;
        let got = fnv64(format!("{:?}", shape(&p, &heap)).as_bytes());
        if got != want {
            differ.push(c.name.as_str());
            if bless {
                println!("blessed: {}", c.name);
                recorded.insert(key, got);
            }
        }
    }
    if bless {
        write_digest(&recorded);
        differ.clear();
    }
    println!("{checked} chunks checked against the digest");
    assert!(
        checked > 0 || !corpus,
        "the corpus shares no chunk with the digest"
    );
    assert!(
        differ.is_empty(),
        "{} chunks compile to another shape than the digest's: {:?}",
        differ.len(),
        &differ[..differ.len().min(20)]
    );
}
