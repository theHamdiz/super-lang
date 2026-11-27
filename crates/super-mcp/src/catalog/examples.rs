//! Example project index under examples/.

use super::ExampleProject;

pub const ALL: &[ExampleProject] = &[
    ExampleProject {
        id: "space-shooter",
        title: "2D Arcade Space Shooter",
        path: "examples/01_space_shooter",
        kind: "game",
        summary: "ECS world, spatial collisions, waves, HUD; tests for damage math.",
        highlights: &["std.game.ecs", "state hot-reload", "spatial grid"],
        snippet: r#"state world = ecs.World{}
update(dt: f32) {
    for @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}
"#,
    },
    ExampleProject {
        id: "task-api",
        title: "REST/JSON Task & Auth API",
        path: "examples/02_task_api",
        kind: "backend",
        summary: "Multi-module Pod with routes, auth, json validation, integration tests.",
        highlights: &["post/get routes", "or err", "std.net.http"],
        snippet: r#"#post("/orders")
handle_orders(req: http.Request) : http.Response! {
    body = req.json(CreateOrder) or err { return http.bad_request(.{error: "{err}"}) }
    return http.ok(.{status: "confirmed"})
}
"#,
    },
    ExampleProject {
        id: "log-analyzer",
        title: "Log & Data Pipeline Analyzer",
        path: "examples/03_log_analyzer",
        kind: "data",
        summary: "Streams structured logs with keep/map/group_by on scoped stacks.",
        highlights: &["UFCS pipelines", "sum types", "zero-copy slices"],
        snippet: r#"errors = rows.keep(it.level == "error").map(it.ms)
"#,
    },
    ExampleProject {
        id: "roguelike",
        title: "Roguelike Dungeon",
        path: "examples/04_roguelike_dungeon",
        kind: "game",
        summary: "Procedural dungeon, FOV, turn-based combat.",
        highlights: &["tilemap", "ai pathfinding"],
        snippet: "// see examples/04_roguelike_dungeon",
    },
    ExampleProject {
        id: "tower-defense",
        title: "Tower Defense",
        path: "examples/05_tower_defense",
        kind: "game",
        summary: "Waves, towers, projectile targeting.",
        highlights: &["ecs", "game.ai"],
        snippet: "// see examples/05_tower_defense",
    },
    ExampleProject {
        id: "physics-sandbox",
        title: "Physics Sandbox",
        path: "examples/06_physics_sandbox",
        kind: "game",
        summary: "2D rigid bodies with impulse resolution.",
        highlights: &["std.game.physics2d"],
        snippet: "// see examples/06_physics_sandbox",
    },
    ExampleProject {
        id: "chat-broker",
        title: "Realtime Chat Broker",
        path: "examples/07_realtime_chat_broker",
        kind: "backend",
        summary: "Fan-out messaging over sockets/WS patterns.",
        highlights: &["std.net.ws", "concurrency"],
        snippet: "// see examples/07_realtime_chat_broker",
    },
    ExampleProject {
        id: "kv-cache",
        title: "KV Cache Engine",
        path: "examples/08_kv_cache_engine",
        kind: "systems",
        summary: "In-memory key-value cache with eviction.",
        highlights: &["lru", "RESP protocols"],
        snippet: "// see examples/08_kv_cache_engine",
    },
    ExampleProject {
        id: "http-load-tester",
        title: "HTTP Load Tester",
        path: "examples/09_http_load_tester",
        kind: "cli",
        summary: "Concurrent HTTP client load generation and stats.",
        highlights: &["std.net.http", "metrics"],
        snippet: "// see examples/09_http_load_tester",
    },
    ExampleProject {
        id: "mapreduce-kv",
        title: "MapReduce KV",
        path: "examples/10_mapreduce_kv",
        kind: "data",
        summary: "Map/reduce over key-value shards.",
        highlights: &["pipelines", "parallelism"],
        snippet: "// see examples/10_mapreduce_kv",
    },
    ExampleProject {
        id: "voxel-raycast",
        title: "Voxel Raycast",
        path: "examples/11_voxel_raycast",
        kind: "game",
        summary: "Grid traversal / grid raycast.",
        highlights: &["math.geom", "algorithms"],
        snippet: "// see examples/11_voxel_raycast",
    },
    ExampleProject {
        id: "boids",
        title: "Boids Flocking",
        path: "examples/12_boids_flocking",
        kind: "game",
        summary: "Steering behaviors for flocking agents.",
        highlights: &["std.game.ai", "ecs"],
        snippet: "// see examples/12_boids_flocking",
    },
    ExampleProject {
        id: "multiplayer-tick",
        title: "Multiplayer Tick",
        path: "examples/13_multiplayer_tick",
        kind: "game",
        summary: "Fixed-tick simulation with netcode packets.",
        highlights: &["std.game.netcode", "state"],
        snippet: "// see examples/13_multiplayer_tick",
    },
    ExampleProject {
        id: "tcp-broker",
        title: "TCP Message Broker",
        path: "examples/14_tcp_message_broker",
        kind: "backend",
        summary: "Length-prefixed TCP framing and routing.",
        highlights: &["std.net.tcp", "std.io.poll"],
        snippet: "// see examples/14_tcp_message_broker",
    },
    ExampleProject {
        id: "http-radix",
        title: "HTTP Radix Framework",
        path: "examples/15_http_radix_framework",
        kind: "backend",
        summary: "Radix-tree HTTP router framework sketch.",
        highlights: &["routing", "http"],
        snippet: "// see examples/15_http_radix_framework",
    },
    ExampleProject {
        id: "resp-lru",
        title: "RESP LRU Cache",
        path: "examples/16_resp_lru_cache",
        kind: "systems",
        summary: "Redis-protocol compatible LRU cache.",
        highlights: &["std.db.redis", "lru"],
        snippet: "// see examples/16_resp_lru_cache",
    },
    ExampleProject {
        id: "log-ingest",
        title: "Log Ingest Pipeline",
        path: "examples/17_log_ingest_pipeline",
        kind: "data",
        summary: "High-throughput log ingest and parse.",
        highlights: &["io", "fmt.json"],
        snippet: "// see examples/17_log_ingest_pipeline",
    },
    ExampleProject {
        id: "rigidbody",
        title: "Rigidbody Sandbox",
        path: "examples/18_rigidbody_sandbox",
        kind: "game",
        summary: "Extended rigid-body playground.",
        highlights: &["physics2d"],
        snippet: "// see examples/18_rigidbody_sandbox",
    },
    ExampleProject {
        id: "ws-multiroom",
        title: "WS Multiroom Chat",
        path: "examples/19_ws_multiroom_chat",
        kind: "backend",
        summary: "WebSocket rooms with fan-out.",
        highlights: &["std.net.ws"],
        snippet: "// see examples/19_ws_multiroom_chat",
    },
    ExampleProject {
        id: "game-server-ws",
        title: "Game Server WS Sync",
        path: "examples/20_game_server_ws_sync",
        kind: "game",
        summary: "Authoritative game server over WebSocket.",
        highlights: &["ws", "netcode", "ecs"],
        snippet: "// see examples/20_game_server_ws_sync",
    },
    ExampleProject {
        id: "cli-ledger",
        title: "CLI Ledger",
        path: "examples/21_cli_ledger",
        kind: "cli",
        summary: "CLI accounting tool.",
        highlights: &["std.cli.cmd"],
        snippet: "// see examples/21_cli_ledger",
    },
    ExampleProject {
        id: "cli-ship",
        title: "CLI Ship",
        path: "examples/22_cli_ship",
        kind: "cli",
        summary: "Shipping/deploy helper CLI.",
        highlights: &["std.cli.cmd", "sys.fs"],
        snippet: "// see examples/22_cli_ship",
    },
    ExampleProject {
        id: "tui-ops",
        title: "TUI Ops Console",
        path: "examples/23_tui_ops",
        kind: "tui",
        summary: "Terminal ops dashboard widgets.",
        highlights: &["std.tui.ui"],
        snippet: "// see examples/23_tui_ops",
    },
    ExampleProject {
        id: "tui-inbox",
        title: "TUI Inbox",
        path: "examples/24_tui_inbox",
        kind: "tui",
        summary: "Keyboard-driven inbox browser.",
        highlights: &["std.tui.ui"],
        snippet: "// see examples/24_tui_inbox",
    },
    ExampleProject {
        id: "gui-studio",
        title: "GUI Studio",
        path: "examples/25_gui_studio",
        kind: "gui",
        summary: "Retained-mode desktop UI studio.",
        highlights: &["std.gui.ui", "std.gui.window"],
        snippet: "// see examples/25_gui_studio",
    },
    ExampleProject {
        id: "gui-atlas",
        title: "GUI Atlas",
        path: "examples/26_gui_atlas",
        kind: "gui",
        summary: "Widget atlas / chrome demos.",
        highlights: &["std.gui.widgets"],
        snippet: "// see examples/26_gui_atlas",
    },
    ExampleProject {
        id: "gui-todo",
        title: "GUI Todo",
        path: "examples/27_gui_todo",
        kind: "gui",
        summary: "Desktop todo app.",
        highlights: &["gui", "widgets"],
        snippet: "// see examples/27_gui_todo",
    },
    ExampleProject {
        id: "gui-breakout",
        title: "GUI Breakout",
        path: "examples/28_gui_breakout",
        kind: "gui",
        summary: "Breakout game on the GUI window presenter.",
        highlights: &["gui.window", "game loop"],
        snippet: "// see examples/28_gui_breakout",
    },
    ExampleProject {
        id: "gui-invaders",
        title: "GUI Invaders",
        path: "examples/29_gui_invaders",
        kind: "gui",
        summary: "arcade GUI game.",
        highlights: &["gui", "input"],
        snippet: "// see examples/29_gui_invaders",
    },
    ExampleProject {
        id: "gui-snake",
        title: "GUI Snake",
        path: "examples/30_gui_snake",
        kind: "gui",
        summary: "Classic snake on std.gui.window.",
        highlights: &["gui.window"],
        snippet: "// see examples/30_gui_snake",
    },
    ExampleProject {
        id: "interop-c",
        title: "C Interop",
        path: "examples/interop_c",
        kind: "interop",
        summary: "Embed Super in C and call a C mathlib from Super via host bindings.",
        highlights: &["super.h", "host.mathlib_lerp"],
        snippet: r#"// Super side:
lerp = host.mathlib_lerp(0.0, 10.0, 0.5)
"#,
    },
    ExampleProject {
        id: "interop-cpp",
        title: "C++ Interop",
        path: "examples/interop_cpp",
        kind: "interop",
        summary: "Embed Super in C++ Engine; call C++ damage helpers from Super.",
        highlights: &["super.hpp", "host.damage_apply_hit"],
        snippet: "// see examples/interop_cpp",
    },
    ExampleProject {
        id: "interop-rust",
        title: "Rust Interop",
        path: "examples/interop_rust",
        kind: "interop",
        summary: "Embed via super_ffi::{Engine, Value}; call host.rarity_score from Super.",
        highlights: &["super_ffi", "never use super::"],
        snippet: r#"use super_ffi::{Engine, Value};
"#,
    },
];

pub fn find(id: &str) -> Option<&'static ExampleProject> {
    ALL.iter()
        .find(|e| e.id == id || e.title.eq_ignore_ascii_case(id) || e.path.ends_with(id))
}

pub fn search(query: &str) -> Vec<&'static ExampleProject> {
    ALL.iter()
        .filter(|e| {
            let blob = format!(
                "{} {} {} {} {} {} {}",
                e.id,
                e.title,
                e.path,
                e.kind,
                e.summary,
                e.highlights.join(" "),
                e.snippet
            );
            super::matches_query(&blob, query)
        })
        .collect()
}
