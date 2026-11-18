//! `membrama serve <options>`: the program the seat starts from its `peers` fact (docs/design.md). It attaches the regions the seat made, loads the model,
//! and answers the seat's requests until the seat's end of the rings is gone (or the process that started it is).
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use membrama::cli::{parse, Config};
use membrama::engine::{EngineGenerator, Runner};
use membrama::peer::Peer;
use membrama::turn::Handler;
use std::process::exit;
use std::time::Duration;

fn fail(why: &str) -> ! { eprintln!("membrama: {why}"); exit(1) }

fn serve(cfg: Config) {
    // the process that must outlive this one: GRAPHLINK_PARENT names it (the seat sets it), else the one that started us
    let parent = if cfg.die_with_parent {
        std::env::var("GRAPHLINK_PARENT").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or_else(std::os::unix::process::parent_id)
    } else { 0 };
    let mut peer = Peer::attach(&cfg.peer, &cfg.ns, &cfg.numbers).unwrap_or_else(|e| fail(&format!("cannot attach the seat's regions: {e}")));
    let backend = LlamaBackend::init().unwrap_or_else(|e| fail(&format!("llama.cpp: {e}")));
    let model = LlamaModel::load_from_file(&backend, &cfg.model, &LlamaModelParams::default().with_n_gpu_layers(99)).unwrap_or_else(|e| fail(&format!("cannot load {}: {e}", cfg.model)));
    let runner = Runner::new(&model, &backend, cfg.n_ctx).unwrap_or_else(|e| fail(&e));
    let mut generator = EngineGenerator::new(runner, &cfg.model, cfg.bank_cap, cfg.max_banks);
    let mut handler = Handler::new();
    eprintln!("membrama: serving {} as {} (model {})", cfg.peer, cfg.ns, cfg.model);
    loop {
        if let Err(e) = handler.tick(&mut peer, &mut generator) { fail(&e); }
        if peer.is_closed() { exit(0); }
        if parent != 0 && std::os::unix::process::parent_id() != parent { exit(0); }
        std::thread::sleep(Duration::from_millis(5));   // the same nap as graphlink-bridge; the seat itself notices a change within 50 ms
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => serve(parse(&args[1..]).unwrap_or_else(|e| fail(&e))),
        _ => fail("usage: membrama serve --peer <name> --ns <namespace> --model <gguf> --n-ctx <n> --slots <n> --key-cap <n> --val-cap <n> --epoch <n> --spins <n> --depth <n> --bank-cap <bytes> --max-banks <n> [--die-with-parent]"),
    }
}
