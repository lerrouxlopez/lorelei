#![forbid(unsafe_code)]

use crate::cli::*;
use crate::harbor_client::HarborClient;
use lorelei_core::config::LoreleiConfig;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command as ProcessCommand;
use uuid::Uuid;

pub async fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Init(args) => match cmd_init(args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Doctor(args) => match cmd_doctor(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Memo(args) => match cmd_memo(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Echo(args) => match cmd_echo(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Ask(args) => match cmd_ask(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Pearls(args) => match cmd_pearls(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Forget(args) => match cmd_forget(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Providers(args) => match cmd_providers(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Shells(args) => match cmd_shells(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Task(args) => match cmd_task(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Approvals(args) => match cmd_approvals(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Approve(args) => match cmd_approve(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Reef { command } => match cmd_reef(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Ship(args) => match cmd_ship(args) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Run(args) => match cmd_run(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Command::Docs(args) => match cmd_docs(args).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
    }
}

fn cmd_init(args: InitArgs) -> Result<(), String> {
    write_from_example("lorelei.toml.example", "lorelei.toml", args.force)?;
    write_from_example(".env.example", ".env", args.force)?;
    println!("Lorelei initialized (The Reef awaits).");
    Ok(())
}

fn write_from_example(example: &str, dest: &str, force: bool) -> Result<(), String> {
    let src = Path::new(example);
    if !src.exists() {
        return Err(format!("missing example file: {example}"));
    }
    let dest = Path::new(dest);
    if dest.exists() && !force {
        println!(
            "skip: `{}` already exists (use --force to overwrite)",
            dest.display()
        );
        return Ok(());
    }
    let bytes = fs::read(src).map_err(|e| format!("failed to read {example}: {e}"))?;
    let mut f =
        fs::File::create(dest).map_err(|e| format!("failed to create {}: {e}", dest.display()))?;
    f.write_all(&bytes)
        .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    Ok(())
}

async fn cmd_doctor(args: ConfigArgs) -> Result<(), String> {
    if !args.config.exists() {
        return Err(format!(
            "doctor: config not found: {}",
            args.config.display()
        ));
    }

    let cfg = LoreleiConfig::load_from_toml_path(&args.config)
        .map_err(|e| format!("doctor: config invalid: {e}"))?;

    let mut problems: Vec<String> = Vec::new();

    // Docker presence (used by `reef` and `ship`)
    if ProcessCommand::new("docker")
        .arg("--version")
        .output()
        .is_err()
    {
        problems.push("docker not found on PATH".to_string());
    }
    if ProcessCommand::new("docker")
        .args(["compose", "version"])
        .output()
        .is_err()
    {
        problems.push("docker compose not available".to_string());
    }

    // Required env vars by config (values not shown)
    check_env(&cfg.lore.postgres_url_env, &mut problems);
    check_env(&cfg.lore.qdrant_url_env, &mut problems);
    for (name, p) in &cfg.providers {
        if std::env::var_os(&p.api_key_env).is_none() {
            problems.push(format!(
                "provider `{name}` missing required env var (value not shown): {}",
                p.api_key_env
            ));
        }
    }

    // Harbor readiness
    let harbor = HarborClient::new(HarborClient::default_base_url(None))?;
    if let Err(e) = harbor.get_status("/healthz").await {
        problems.push(format!("harbor healthz failed: {e}"));
    }
    if let Err(e) = harbor.get_status("/readyz").await {
        problems.push(format!("harbor readyz failed: {e}"));
    }

    if problems.is_empty() {
        println!("doctor ok: Reef is ready.");
        Ok(())
    } else {
        let mut out = String::new();
        out.push_str("doctor: issues found:\n");
        for p in problems {
            out.push_str("- ");
            out.push_str(&p);
            out.push('\n');
        }
        Err(out.trim_end().to_string())
    }
}

#[derive(Debug, Serialize)]
struct CreatePearlRequest {
    tenant_id: Uuid,
    agent_id: Uuid,
    pearl_type: Option<lorelei_core::types::PearlType>,
    content: String,
    confidence: Option<f64>,
    importance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PearlResponse {
    pearl_id: Uuid,
    tenant_id: Uuid,
    agent_id: Uuid,
    pearl_type: lorelei_core::types::PearlType,
    content: String,
    confidence: f64,
    importance: f64,
    created_at: String,
}

async fn cmd_memo(args: MemoArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let pearl_type = match args.pearl_type {
        Some(s) => Some(parse_pearl_type(&s)?),
        None => None,
    };

    let created: PearlResponse = harbor
        .post_json(
            "/v1/pearls",
            &CreatePearlRequest {
                tenant_id: cfg.agent.tenant_id.0,
                agent_id: cfg.agent.agent_id.0,
                pearl_type,
                content: args.content,
                confidence: args.confidence,
                importance: args.importance,
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    println!("saved pearl: {}", created.pearl_id);
    Ok(())
}

#[derive(Debug, Serialize)]
struct EchoRequest {
    tenant_id: Uuid,
    agent_id: Uuid,
    query: String,
    top_k: Option<usize>,
    min_confidence: Option<f64>,
    pearl_type: Option<lorelei_core::types::PearlType>,
}

#[derive(Debug, Deserialize)]
struct EchoHitResponse {
    score: lorelei_core::types::UnitInterval,
    pearl_id: lorelei_core::types::PearlId,
    content: String,
    pearl_type: lorelei_core::types::PearlType,
    reason: String,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct CreateRunRequest {
    tenant_id: Uuid,
    agent_id: Uuid,
    input: String,
    #[serde(rename = "async", skip_serializing_if = "Option::is_none")]
    async_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    no_memory: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RunResponse {
    run_id: Uuid,
    status: lorelei_core::types::RunStatus,
    output: Option<String>,
}

async fn cmd_echo(args: EchoArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let pearl_type = match args.pearl_type {
        Some(s) => Some(parse_pearl_type(&s)?),
        None => None,
    };

    let hits: Vec<EchoHitResponse> = harbor
        .post_json(
            "/v1/echo",
            &EchoRequest {
                tenant_id: cfg.agent.tenant_id.0,
                agent_id: cfg.agent.agent_id.0,
                query: args.query,
                top_k: args.top_k,
                min_confidence: args.min_confidence,
                pearl_type,
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    if hits.is_empty() {
        println!("(no echoes)");
        return Ok(());
    }

    for h in hits {
        println!(
            "{:.4}\t{}\t{:?}\t{}\t{}\t{}",
            h.score.get(),
            h.pearl_id.0,
            h.pearl_type,
            h.created_at,
            h.reason,
            h.content
        );
    }
    Ok(())
}

async fn cmd_ask(args: AskArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let tenant_id = cfg.agent.tenant_id.0;
    let agent_id = cfg.agent.agent_id.0;

    let created: RunResponse = harbor
        .post_json(
            "/v1/runs",
            &CreateRunRequest {
                tenant_id,
                agent_id,
                input: args.prompt,
                async_run: if args.progress { Some(true) } else { None },
                no_memory: if args.no_memory { Some(true) } else { None },
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    if !args.progress {
        let output = created.output.unwrap_or_default();
        println!("{output}");
        return Ok(());
    }

    println!("run_id={}", created.run_id);

    let mut last_event_count: usize = 0;
    let mut printed_final_answer = false;
    loop {
        let run_path = format!(
            "/v1/runs/{}?tenant_id={}&agent_id={}",
            created.run_id, tenant_id, agent_id
        );
        let run: RunInspectResponse =
            harbor.get_json(&run_path).await.map_err(|e| e.to_string())?;

        let currents_path = format!(
            "/v1/runs/{}/currents?tenant_id={}&agent_id={}",
            created.run_id, tenant_id, agent_id
        );
        let events: Vec<CurrentEventResponse> =
            harbor.get_json(&currents_path).await.map_err(|e| e.to_string())?;

        if events.len() > last_event_count {
            for e in events.iter().skip(last_event_count) {
                match e.event_type {
                    lorelei_core::types::CurrentEventType::Assistant if e.summary == "final answer" => {
                        if let Some(t) = e.data.get("text").and_then(|v| v.as_str()) {
                            println!("{t}");
                            printed_final_answer = true;
                        } else {
                            println!("{}", dim_grey(&format!("{}\t{:?}\t{}", e.created_at, e.event_type, e.summary)));
                        }
                    }
                    lorelei_core::types::CurrentEventType::System => {
                        // "Thinking" progress: keep it subtle so the user knows we're working.
                        println!("{}", dim_grey(&format!("· {}", e.summary)));
                    }
                    lorelei_core::types::CurrentEventType::User => {
                        println!("{}", dim_grey(&format!("> {}", e.summary)));
                    }
                    _ => println!("{}", dim_grey(&format!("{}\t{:?}\t{}", e.created_at, e.event_type, e.summary))),
                }
            }
            last_event_count = events.len();
        }

        if printed_final_answer {
            // Return control to the user as soon as the answer is available.
            // Memory formation and indexing continue server-side.
            break;
        }

        if run.status != lorelei_core::types::RunStatus::Running {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Ok(())
}

fn dim_grey(s: &str) -> String {
    // Bright black (90) + dim (2). Resets at end.
    // If the terminal doesn't support ANSI, this will display raw codes; users can set `--progress false`.
    format!("\x1b[2;90m{s}\x1b[0m")
}

async fn cmd_pearls(args: PearlsArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let mut path = format!("/v1/pearls?tenant_id={}", cfg.agent.tenant_id.0);
    if let Some(a) = args.agent_id {
        path.push_str("&agent_id=");
        path.push_str(&a);
    }
    if let Some(t) = args.pearl_type {
        path.push_str("&pearl_type=");
        path.push_str(&t);
    }
    if let Some(l) = args.limit {
        path.push_str("&limit=");
        path.push_str(&l.to_string());
    }

    let pearls: Vec<PearlResponse> = harbor.get_json(&path).await.map_err(|e| e.to_string())?;

    for p in pearls {
        println!("{}\t{:?}\t{}", p.pearl_id, p.pearl_type, p.content);
    }
    Ok(())
}

async fn cmd_forget(args: ForgetArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let pearl_id = Uuid::parse_str(&args.pearl_id).map_err(|_| "invalid pearl_id".to_string())?;
    let path = format!("/v1/pearls/{pearl_id}?tenant_id={}", cfg.agent.tenant_id.0);
    harbor
        .delete_empty(&path)
        .await
        .map_err(|e| e.to_string())?;
    println!("forgot pearl: {pearl_id}");
    Ok(())
}

async fn cmd_providers(args: HarborArgs) -> Result<(), String> {
    let harbor_url = HarborClient::default_base_url(args.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;
    let providers: serde_json::Value = harbor
        .get_json("/v1/providers")
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&providers).unwrap_or_else(|_| "[]".to_string())
    );
    Ok(())
}

async fn cmd_shells(args: HarborArgs) -> Result<(), String> {
    let harbor_url = HarborClient::default_base_url(args.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;
    let shells: serde_json::Value = harbor
        .get_json("/v1/shells")
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&shells).unwrap_or_else(|_| "[]".to_string())
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TaskResponse {
    task_id: Uuid,
    tenant_id: Uuid,
    agent_id: Uuid,
    prompt: String,
    status: lorelei_core::types::TaskStatus,
    schedule: lorelei_core::types::TaskSchedule,
    next_run_at: String,
    last_run_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct CreateTaskRequest {
    tenant_id: Uuid,
    agent_id: Uuid,
    prompt: String,
    daily: bool,
    at: Option<String>,
}

async fn cmd_task(args: TaskArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    match args.command {
        TaskCommand::Add(a) => {
            let task: TaskResponse = harbor
                .post_json(
                    "/v1/tasks",
                    &CreateTaskRequest {
                        tenant_id: cfg.agent.tenant_id.0,
                        agent_id: cfg.agent.agent_id.0,
                        prompt: a.prompt,
                        daily: a.daily,
                        at: a.at,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "task {}\t{:?}\t{}\t{}",
                task.task_id, task.status, task.next_run_at, task.prompt
            );
            Ok(())
        }
        TaskCommand::List(a) => {
            let mut path = format!("/v1/tasks?tenant_id={}", cfg.agent.tenant_id.0);
            if let Some(agent_id) = a.agent_id {
                path.push_str("&agent_id=");
                path.push_str(&agent_id);
            }
            let tasks: Vec<TaskResponse> =
                harbor.get_json(&path).await.map_err(|e| e.to_string())?;
            for t in tasks {
                println!(
                    "{}\t{:?}\t{}\t{}",
                    t.task_id, t.status, t.next_run_at, t.prompt
                );
            }
            Ok(())
        }
        TaskCommand::Pause(a) => {
            let tenant = cfg.agent.tenant_id.0;
            let path = format!("/v1/tasks/{}/pause?tenant_id={}", a.task_id, tenant);
            let _: serde_json::Value = harbor
                .post_json(&path, &serde_json::json!({}))
                .await
                .map_err(|e| e.to_string())?;
            println!("paused task: {}", a.task_id);
            Ok(())
        }
        TaskCommand::Resume(a) => {
            let tenant = cfg.agent.tenant_id.0;
            let path = format!("/v1/tasks/{}/resume?tenant_id={}", a.task_id, tenant);
            let _: serde_json::Value = harbor
                .post_json(&path, &serde_json::json!({}))
                .await
                .map_err(|e| e.to_string())?;
            println!("resumed task: {}", a.task_id);
            Ok(())
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ApprovalResponse {
    approval_id: Uuid,
    tenant_id: Uuid,
    agent_id: Uuid,
    task_id: Option<Uuid>,
    run_id: Uuid,
    tool: String,
    risk: lorelei_core::types::ShellRisk,
    state: lorelei_core::types::ApprovalState,
    approval_prompt: String,
    created_at: String,
    decided_at: Option<String>,
}

async fn cmd_approvals(args: ApprovalsArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let mut path = format!("/v1/approvals?tenant_id={}", cfg.agent.tenant_id.0);
    if let Some(state) = args.state {
        path.push_str("&state=");
        path.push_str(&state);
    }
    let approvals: Vec<ApprovalResponse> =
        harbor.get_json(&path).await.map_err(|e| e.to_string())?;
    for a in approvals {
        println!(
            "{}\t{:?}\t{:?}\t{}\t{}",
            a.approval_id, a.state, a.risk, a.tool, a.created_at
        );
    }
    Ok(())
}

async fn cmd_approve(args: ApproveArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config invalid: {e}"))?;

    let harbor_url = HarborClient::default_base_url(args.harbor.harbor_url);
    let harbor = HarborClient::new(harbor_url)?;

    let tenant = cfg.agent.tenant_id.0;
    let path = format!(
        "/v1/approvals/{}/approve?tenant_id={}",
        args.approval_id, tenant
    );
    let _: serde_json::Value = harbor
        .post_json(&path, &serde_json::json!({}))
        .await
        .map_err(|e| e.to_string())?;
    println!("approved: {}", args.approval_id);
    Ok(())
}

fn cmd_reef(cmd: ReefCommand) -> Result<i32, String> {
    match cmd {
        ReefCommand::Up(args) => run_compose(
            &args.common.compose_file,
            &[
                "up",
                "-d",
                "postgres",
                "qdrant",
                "ollama",
                "ollama-pull",
                "harbor",
            ],
        ),
        ReefCommand::Down(args) => run_compose(&args.compose_file, &["down"]),
        ReefCommand::Logs(args) => {
            let mut parts: Vec<String> = vec![
                "logs".to_string(),
                "--tail".to_string(),
                args.tail.to_string(),
                "harbor".to_string(),
                "postgres".to_string(),
                "qdrant".to_string(),
                "ollama".to_string(),
            ];
            if args.follow {
                parts.insert(1, "-f".to_string());
            }
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            run_compose(&args.common.compose_file, &refs)
        }
    }
}

fn cmd_ship(args: ShipArgs) -> Result<i32, String> {
    let code = run_compose(&args.compose_file, &["build", "harbor"])?;

    // Build the CLI image too (for convenient local use / CI).
    // This is intentionally independent from `docker compose`.
    let mut lore = ProcessCommand::new("docker");
    lore.args([
        "build",
        "-f",
        "docker/Dockerfile.lore",
        "-t",
        "lorelei-cli:latest",
        ".",
    ]);
    println!("docker build -f docker/Dockerfile.lore -t lorelei-cli:latest .");
    let status = lore
        .status()
        .map_err(|e| format!("failed to run `docker build` for lore image: {e}"))?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1));
    }

    Ok(code)
}

#[derive(Debug, Deserialize)]
struct CurrentEventResponse {
    event_type: lorelei_core::types::CurrentEventType,
    created_at: String,
    summary: String,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RunInspectResponse {
    run_id: Uuid,
    tenant_id: Uuid,
    agent_id: Uuid,
    status: lorelei_core::types::RunStatus,
    output: Option<String>,
}

async fn cmd_run(args: RunArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config error: {e}"))?;
    let harbor = HarborClient::new(HarborClient::default_base_url(args.harbor.harbor_url))?;
    let tenant = cfg.agent.tenant_id.0;
    let agent = cfg.agent.agent_id.0;

    match args.command {
        RunCommand::Inspect(r) => {
            let path = format!(
                "/v1/runs/{}?tenant_id={}&agent_id={}",
                r.run_id, tenant, agent
            );
            let run: RunInspectResponse =
                harbor.get_json(&path).await.map_err(|e| e.to_string())?;
            println!(
                "run {}\tstatus={:?}\ttenant={}\tagent={}",
                run.run_id, run.status, run.tenant_id, run.agent_id
            );
            if let Some(out) = run.output {
                if !out.trim().is_empty() {
                    println!("{out}");
                }
            }
        }
        RunCommand::Currents(r) => {
            let path = format!(
                "/v1/runs/{}/currents?tenant_id={}&agent_id={}",
                r.run_id, tenant, agent
            );
            let events: Vec<CurrentEventResponse> =
                harbor.get_json(&path).await.map_err(|e| e.to_string())?;
            for e in events {
                println!("{}\t{:?}\t{}", e.created_at, e.event_type, e.summary);
            }
        }
        RunCommand::Memories(r) => {
            let path = format!(
                "/v1/runs/{}/currents?tenant_id={}&agent_id={}",
                r.run_id, tenant, agent
            );
            let events: Vec<CurrentEventResponse> =
                harbor.get_json(&path).await.map_err(|e| e.to_string())?;

            let mut pearl_ids: Vec<String> = Vec::new();
            for e in events {
                if e.summary != "memory formation" {
                    continue;
                }
                if let Some(accepted) = e
                    .data
                    .get("decisions")
                    .and_then(|d| d.get("accepted"))
                    .and_then(|a| a.as_array())
                {
                    for a in accepted {
                        if let Some(id) = a.get("pearl_id").and_then(|v| v.as_str()) {
                            pearl_ids.push(id.to_string());
                        }
                    }
                }
            }
            pearl_ids.sort();
            pearl_ids.dedup();

            if pearl_ids.is_empty() {
                println!("(no accepted Pearls recorded for this run)");
                return Ok(());
            }

            for pid in pearl_ids {
                let p = format!("/v1/pearls/{pid}?tenant_id={tenant}");
                let pearl: PearlResponse = harbor.get_json(&p).await.map_err(|e| e.to_string())?;
                println!(
                    "{}\t{:?}\t{}",
                    pearl.pearl_id, pearl.pearl_type, pearl.content
                );
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct DocsIngestRequest {
    tenant_id: Uuid,
    agent_id: Uuid,
    path: String,
}

#[derive(Debug, Deserialize)]
struct DocsIngestResponse {
    document_id: Uuid,
}

async fn cmd_docs(args: DocsArgs) -> Result<(), String> {
    let cfg = LoreleiConfig::load_from_toml_path(&args.config.config)
        .map_err(|e| format!("config error: {e}"))?;
    let harbor = HarborClient::new(HarborClient::default_base_url(args.harbor.harbor_url))?;

    let tenant = cfg.agent.tenant_id.0;
    let agent = cfg.agent.agent_id.0;

    match args.command {
        DocsCommand::Ingest(i) => {
            let res: DocsIngestResponse = harbor
                .post_json(
                    "/v1/docs/ingest",
                    &DocsIngestRequest {
                        tenant_id: tenant,
                        agent_id: agent,
                        path: i.path,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            println!("document_id={}", res.document_id);
        }
        DocsCommand::Search(s) => {
            #[derive(Debug, Serialize)]
            struct EchoReq {
                tenant_id: Uuid,
                agent_id: Uuid,
                query: String,
                top_k: Option<usize>,
                min_confidence: Option<f64>,
                pearl_type: Option<lorelei_core::types::PearlType>,
                sources: Option<String>,
            }
            let hits: Vec<lorelei_core::types::EchoHit> = harbor
                .post_json(
                    "/v1/echo",
                    &EchoReq {
                        tenant_id: tenant,
                        agent_id: agent,
                        query: s.query,
                        top_k: s.top_k,
                        min_confidence: None,
                        pearl_type: None,
                        sources: Some("documents".to_string()),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;

            for h in hits {
                if let Some(c) = h.citation {
                    println!(
                        "{}\t{}\t#{}\t{}",
                        h.score.get(),
                        c.title,
                        c.chunk_index,
                        h.content.replace('\n', " ")
                    );
                } else {
                    println!("{}\t{}", h.score.get(), h.content.replace('\n', " "));
                }
            }
        }
    }

    Ok(())
}

fn run_compose(compose_file: &Path, args: &[&str]) -> Result<i32, String> {
    let mut cmd = ProcessCommand::new("docker");
    cmd.arg("compose").arg("-f").arg(compose_file);
    cmd.args(args);

    let pretty = format!(
        "docker compose -f {} {}",
        compose_file.display(),
        args.join(" ")
    );
    println!("{pretty}");

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run `{pretty}`: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

fn check_env(key: &str, problems: &mut Vec<String>) {
    if std::env::var_os(key).is_none() {
        problems.push(format!("missing required env var (value not shown): {key}"));
    }
}

fn parse_pearl_type(s: &str) -> Result<lorelei_core::types::PearlType, String> {
    let normalized = s.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "semantic" => Ok(lorelei_core::types::PearlType::Fact),
        "fact" => Ok(lorelei_core::types::PearlType::Fact),
        "preference" => Ok(lorelei_core::types::PearlType::Preference),
        "skill" => Ok(lorelei_core::types::PearlType::Skill),
        "plan" => Ok(lorelei_core::types::PearlType::Plan),
        "other" => Ok(lorelei_core::types::PearlType::Other),
        _ => Err("invalid pearl_type (try: semantic|fact|preference|skill|plan|other)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;
    use std::sync::Mutex;

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn init_does_not_overwrite_without_force() {
        let _guard = CWD_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        fs::write("lorelei.toml.example", "example").unwrap();
        fs::write(".env.example", "example").unwrap();
        fs::write("lorelei.toml", "existing").unwrap();

        cmd_init(InitArgs { force: false }).unwrap();
        assert_eq!(fs::read_to_string("lorelei.toml").unwrap(), "existing");
    }

    #[tokio::test]
    async fn doctor_reports_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let err = cmd_doctor(ConfigArgs { config: missing })
            .await
            .unwrap_err();
        assert!(err.contains("config not found"));
    }

    #[tokio::test]
    async fn http_errors_are_presented_clearly() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/providers");
            then.status(404).body("{}");
        });

        let harbor = HarborClient::new(server.url("")).unwrap();
        let err = harbor
            .get_json::<serde_json::Value>("/v1/providers")
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("404"));
        assert!(msg.contains("/v1/providers"));
    }
}
