use std::sync::Arc;
use std::path::Path;

use chrono::Utc;
use lorelei_core::{
    CurrentEvent, EchoQuery, LoreleiError, NewPearl, ProposedAction, ShellCall, ShellRisk,
    SirenPolicy, SongChunk, SongProvider, SongRequest,
};
use async_trait::async_trait;
use lorelei_echo::EchoService;
use lorelei_lore::LoreStores;
use lorelei_shells::ShellRegistryPg;
use lorelei_siren::{DeterministicSirenPolicy, SirenConfig, SirenPrompt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tracing::{info, info_span, Instrument};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TideConfig {
    pub siren: SirenConfig,
    pub siren_prompt_path: Option<String>,
    pub lore_extractor_path: String,
    pub lore_critic_path: String,
}

impl Default for TideConfig {
    fn default() -> Self {
        Self {
            siren: SirenConfig::default(),
            siren_prompt_path: Some("prompts/siren_policy.md".to_string()),
            lore_extractor_path: "prompts/lore_extractor.md".to_string(),
            lore_critic_path: "prompts/lore_critic.md".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    #[serde(default)]
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanStep {
    Noop { message: String },
    Shell {
        name: String,
        input: JsonValue,
        #[serde(default)]
        rationale: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TideOutput {
    pub run_id: Uuid,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRowLite {
    pub id: Uuid,
}

#[async_trait]
pub trait LoreRuntime: Send + Sync {
    async fn create_run(&self, tenant_id: Uuid, metadata: JsonValue) -> Result<RunRowLite, LoreleiError>;
    async fn complete_run(&self, tenant_id: Uuid, run_id: Uuid, ended_at: chrono::DateTime<Utc>) -> Result<(), LoreleiError>;
    async fn write_current(
        &self,
        tenant_id: Uuid,
        event: CurrentEvent,
        confidence: Option<f64>,
        importance: Option<f64>,
    ) -> Result<(), LoreleiError>;
    async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<lorelei_core::Pearl, LoreleiError>;
}

#[async_trait]
impl LoreRuntime for LoreStores {
    async fn create_run(&self, tenant_id: Uuid, metadata: JsonValue) -> Result<RunRowLite, LoreleiError> {
        let r = LoreStores::create_run(self, tenant_id, metadata).await?;
        Ok(RunRowLite { id: r.id })
    }

    async fn complete_run(&self, tenant_id: Uuid, run_id: Uuid, ended_at: chrono::DateTime<Utc>) -> Result<(), LoreleiError> {
        LoreStores::complete_run(self, tenant_id, run_id, ended_at).await
    }

    async fn write_current(
        &self,
        tenant_id: Uuid,
        event: CurrentEvent,
        confidence: Option<f64>,
        importance: Option<f64>,
    ) -> Result<(), LoreleiError> {
        LoreStores::write_current(self, tenant_id, event, confidence, importance).await
    }

    async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<lorelei_core::Pearl, LoreleiError> {
        LoreStores::save_pearl(self, tenant_id, agent_id, run_id, pearl).await
    }
}

#[async_trait]
pub trait EchoRuntime: Send + Sync {
    async fn retrieve(&self, q: EchoQuery) -> Result<Vec<lorelei_core::EchoHit>, LoreleiError>;
}

#[async_trait]
impl EchoRuntime for EchoService {
    async fn retrieve(&self, q: EchoQuery) -> Result<Vec<lorelei_core::EchoHit>, LoreleiError> {
        EchoService::retrieve(self, q).await
    }
}

#[async_trait]
impl EchoRuntime for Arc<EchoService> {
    async fn retrieve(&self, q: EchoQuery) -> Result<Vec<lorelei_core::EchoHit>, LoreleiError> {
        EchoService::retrieve(self.as_ref(), q).await
    }
}

pub struct TideEngine<L: LoreRuntime, E: EchoRuntime, S: SongProvider> {
    lore: L,
    echo: E,
    song: S,
    shells: Arc<dyn ShellRuntime>,
    siren: Arc<dyn SirenPolicy>,
    cfg: TideConfig,
}

impl<L: LoreRuntime, E: EchoRuntime, S: SongProvider> TideEngine<L, E, S> {
    pub fn new(
        lore: L,
        echo: E,
        song: S,
        shells: Arc<dyn ShellRuntime>,
        cfg: TideConfig,
    ) -> Result<Self, LoreleiError> {
        let mut siren_impl = DeterministicSirenPolicy::new(cfg.siren.clone());
        if let Some(path) = &cfg.siren_prompt_path {
            if let Ok(p) = SirenPrompt::load(path) {
                siren_impl = siren_impl.with_prompt(p);
            }
        }
        Ok(Self {
            lore,
            echo,
            song,
            shells,
            siren: Arc::new(siren_impl),
            cfg,
        })
    }

    pub async fn run(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        user_text: String,
    ) -> Result<TideOutput, LoreleiError> {
        let run = self
            .create_run(tenant_id)
            .instrument(info_span!("tide.create_run"))
            .await?;
        let run_id = run.id;

        let run_span = info_span!("tide.run", run_id = %run_id, tenant_id = %tenant_id, agent_id = ?agent_id);
        let _run_guard = run_span.enter();

        self.write_user_event(tenant_id, run_id, &user_text)
            .instrument(info_span!("tide.write_user_current"))
            .await?;

        let echoes = self
            .echo
            .retrieve(EchoQuery {
                text: user_text.clone(),
                tenant_id,
                agent_id,
                run_id: Some(run_id),
                pearl_type: None,
                min_confidence: None,
                limit: 10,
            })
            .instrument(info_span!("tide.echo"))
            .await?;

        info!(
            echo_query_count = 1u64,
            echo_hit_count = echoes.len() as u64,
            "tide.echo_summary"
        );

        let plan = self
            .plan(tenant_id, agent_id, run_id, &user_text, &echoes)
            .instrument(info_span!("tide.plan"))
            .await?;

        let mut last_shell_summary: Option<String> = None;
        for (i, step) in plan.steps.iter().enumerate() {
            match step {
                PlanStep::Noop { .. } => {}
                PlanStep::Shell { name, input, rationale } => {
                    let span = info_span!("tide.shell_step", step_index = i, shell = name);
                    let res = self
                        .run_shell_step(tenant_id, run_id, name, input.clone(), rationale.clone())
                        .instrument(span)
                        .await?;
                    last_shell_summary = Some(res);
                }
            }
        }

        let answer = self
            .final_answer(tenant_id, agent_id, run_id, &user_text, &echoes, last_shell_summary)
            .instrument(info_span!("tide.final_answer"))
            .await?;

        self.write_shell_or_answer_event(tenant_id, run_id, "final_answer", json!({"summary": truncate(&answer, 800)}))
            .instrument(info_span!("tide.write_final_current"))
            .await?;

        let candidates = self
            .extract_pearls(&user_text, &answer)
            .instrument(info_span!("tide.lore_extract"))
            .await?;
        let accepted = self
            .critique_pearls(&candidates)
            .instrument(info_span!("tide.lore_critic"))
            .await?;

        for (pearl, accept) in candidates.into_iter().zip(accepted.into_iter()) {
            if !accept.accept {
                continue;
            }
            let saved = self
                .lore
                .save_pearl(tenant_id, agent_id, run_id, pearl)
                .instrument(info_span!("tide.save_pearl"))
                .await?;
            // Vector indexing requires embeddings; use echo embedder config indirectly isn't accessible.
            // TODO: embed saved.content and upsert_pearl_vector when embedder is available here.
            let _ = saved;
        }

        self.lore
            .complete_run(tenant_id, run_id, Utc::now())
            .instrument(info_span!("tide.complete_run"))
            .await?;

        Ok(TideOutput { run_id, answer })
    }

    async fn create_run(&self, tenant_id: Uuid) -> Result<RunRowLite, LoreleiError> {
        self.lore.create_run(tenant_id, json!({ "component": "tide" })).await
    }

    async fn write_user_event(&self, tenant_id: Uuid, run_id: Uuid, text: &str) -> Result<(), LoreleiError> {
        let event = CurrentEvent {
            id: Uuid::new_v4(),
            run_id,
            at: Utc::now(),
            kind: "user".to_string(),
            payload: json!({ "text": truncate(text, 2000) }),
        };
        self.lore.write_current(tenant_id, event, None, None).await
    }

    async fn write_shell_or_answer_event(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        kind: &str,
        payload: JsonValue,
    ) -> Result<(), LoreleiError> {
        let event = CurrentEvent {
            id: Uuid::new_v4(),
            run_id,
            at: Utc::now(),
            kind: kind.to_string(),
            payload,
        };
        self.lore.write_current(tenant_id, event, None, None).await
    }

    async fn plan(
        &self,
        _tenant_id: Uuid,
        _agent_id: Option<Uuid>,
        _run_id: Uuid,
        user_text: &str,
        echoes: &[lorelei_core::EchoHit],
    ) -> Result<Plan, LoreleiError> {
        let prompt = format!(
            "You are Lorelei Tide planner. Return strict JSON only matching this schema:\n\
             {{\"steps\":[{{\"type\":\"noop\",\"message\":\"...\"}}|{{\"type\":\"shell\",\"name\":\"echo\",\"input\":{{...}},\"rationale\":\"...\"}}]}}\n\
             User: {user_text}\n\
             Echo hits: {}\n\
             If no shell is needed, return a single noop.",
            serde_json::to_string(echoes).unwrap_or_default()
        );

        let json_text = self.ask_json(prompt).await?;
        match parse_plan_anywhere(&json_text) {
            Ok(v) => Ok(v),
            Err(_) => self.repair_plan_once(&json_text).await,
        }
    }

    async fn run_shell_step(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        shell_name: &str,
        input: JsonValue,
        rationale: String,
    ) -> Result<String, LoreleiError> {
        self.shells.validate_name(shell_name)?;
        let risk = self.shells.risk(shell_name)?;
        info!(
            tenant_id = %tenant_id,
            run_id = %run_id,
            shell_name,
            shell_risk = ?risk,
            "tide.shell_classified"
        );

        let action = ProposedAction {
            tenant_id,
            target_tenant_id: tenant_id,
            call: ShellCall {
                program: shell_name.to_string(),
                args: vec![],
                cwd: None,
                env: Default::default(),
                stdin: None,
                timeout_ms: Some(30_000),
            },
            risk,
            rationale,
        };

        let decision = self.siren.decide(action.clone()).await?;
        info!(
            tenant_id = %tenant_id,
            run_id = %run_id,
            shell_name,
            shell_risk = ?decision.risk,
            siren_allow = decision.allow,
            "tide.siren_decision"
        );
        self.write_shell_or_answer_event(
            tenant_id,
            run_id,
            "siren_decision",
            json!({"allow": decision.allow, "risk": format!("{:?}", decision.risk).to_lowercase(), "reasons": decision.reasons}),
        )
        .await?;

        if !decision.allow {
            return Ok("shell not executed (policy denied or needs approval)".to_string());
        }

        let res = self
            .shells
            .execute(
                tenant_id,
                run_id,
                shell_name,
                action.call.clone(),
                input,
            )
            .await?;

        self.write_shell_or_answer_event(
            tenant_id,
            run_id,
            "shell_result",
            json!({
                "shell": shell_name,
                "exit_code": res.exit_code,
                "stdout_summary": truncate(&res.stdout, 800),
                "stderr_summary": truncate(&res.stderr, 800),
            }),
        )
        .await?;

        Ok(format!("shell `{shell_name}` exit_code={}", res.exit_code))
    }

    async fn final_answer(
        &self,
        _tenant_id: Uuid,
        _agent_id: Option<Uuid>,
        _run_id: Uuid,
        user_text: &str,
        echoes: &[lorelei_core::EchoHit],
        shell_summary: Option<String>,
    ) -> Result<String, LoreleiError> {
        let prompt = format!(
            "Answer the user. Use echo hits as supporting context. Do not reveal hidden reasoning.\n\
             User: {user_text}\n\
             Echo hits: {}\n\
             Shell summary: {}\n",
            serde_json::to_string(echoes).unwrap_or_default(),
            shell_summary.unwrap_or_default()
        );

        let resp = self
            .song
            .song(SongRequest {
                prompt,
                context: vec![],
                max_chunks: None,
                parameters: Default::default(),
            })
            .await?;
        Ok(join_chunks(&resp.chunks))
    }

    async fn extract_pearls(&self, user_text: &str, answer: &str) -> Result<Vec<NewPearl>, LoreleiError> {
        let prompt = format!(
            "{}\n\nUser:\n{}\n\nAssistant answer:\n{}\n",
            read_prompt(&self.cfg.lore_extractor_path)?,
            user_text,
            answer
        );
        let json_text = self.ask_json(prompt).await?;
        let parsed = match parse_json_anywhere::<Vec<NewPearl>>(&json_text) {
            Ok(v) => v,
            Err(_) => self.repair_json_once::<Vec<NewPearl>>(&json_text, "PearlList").await?,
        };

        // Local models frequently emit partial/empty fields; treat invalid pearls as "no-op"
        // rather than failing the whole run.
        let mut out = Vec::new();
        for p in parsed {
            if let Err(e) = p.validate() {
                tracing::warn!(error=%e, "tide.extract_pearls.invalid_skipped");
                continue;
            }
            out.push(p);
        }
        Ok(out)
    }

    async fn critique_pearls(&self, pearls: &[NewPearl]) -> Result<Vec<CritiqueDecision>, LoreleiError> {
        let prompt = format!(
            "{}\n\nPearls:\n{}\n",
            read_prompt(&self.cfg.lore_critic_path)?,
            serde_json::to_string(pearls).unwrap_or_default()
        );
        let json_text = self.ask_json(prompt).await?;
        match parse_json_anywhere::<Vec<CritiqueDecision>>(&json_text) {
            Ok(v) => Ok(v),
            Err(_) => self
                .repair_json_once::<Vec<CritiqueDecision>>(&json_text, "CritiqueList")
                .await,
        }
    }

    async fn ask_json(&self, prompt: String) -> Result<String, LoreleiError> {
        let resp = self
            .song
            .song(SongRequest {
                prompt,
                context: vec![],
                max_chunks: None,
                parameters: Default::default(),
            })
            .await?;
        let out = join_chunks(&resp.chunks);
        if out.trim().is_empty() {
            return Err(LoreleiError::validation(
                "song provider returned empty text (expected JSON)".to_string(),
            ));
        }
        Ok(out)
    }

    async fn repair_json_once<T: for<'de> Deserialize<'de>>(
        &self,
        bad: &str,
        type_name: &str,
    ) -> Result<T, LoreleiError> {
        let prompt = format!(
            "Repair the following into strict JSON for type {type_name}. Return JSON only.\n\n{bad}"
        );
        let repaired = self.ask_json(prompt).await?;
        parse_json_anywhere::<T>(&repaired)
    }

    async fn repair_plan_once(&self, bad: &str) -> Result<Plan, LoreleiError> {
        let prompt = format!(
            "Repair the following into strict JSON matching exactly this schema:\n\
             {{\"steps\":[{{\"type\":\"noop\",\"message\":\"...\"}}|{{\"type\":\"shell\",\"name\":\"...\",\"input\":{{...}},\"rationale\":\"...\"}}]}}\n\
             Return JSON only (top-level object with a `steps` array).\n\n{bad}"
        );
        let repaired = self.ask_json(prompt).await?;
        parse_plan_anywhere(&repaired)
    }
}

#[async_trait]
pub trait ShellRuntime: Send + Sync {
    fn validate_name(&self, name: &str) -> Result<(), LoreleiError>;
    fn risk(&self, name: &str) -> Result<ShellRisk, LoreleiError>;
    async fn execute(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        shell_name: &str,
        call: ShellCall,
        input: JsonValue,
    ) -> Result<lorelei_core::ShellResult, LoreleiError>;
}

#[async_trait]
impl ShellRuntime for ShellRegistryPg {
    fn validate_name(&self, name: &str) -> Result<(), LoreleiError> {
        ShellRegistryPg::validate_name(self, name)
    }

    fn risk(&self, name: &str) -> Result<ShellRisk, LoreleiError> {
        ShellRegistryPg::risk(self, name)
    }

    async fn execute(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        shell_name: &str,
        call: ShellCall,
        input: JsonValue,
    ) -> Result<lorelei_core::ShellResult, LoreleiError> {
        ShellRegistryPg::execute(self, tenant_id, run_id, shell_name, call, input).await
    }
}

#[async_trait]
impl ShellRuntime for Arc<ShellRegistryPg> {
    fn validate_name(&self, name: &str) -> Result<(), LoreleiError> {
        ShellRegistryPg::validate_name(self.as_ref(), name)
    }

    fn risk(&self, name: &str) -> Result<ShellRisk, LoreleiError> {
        ShellRegistryPg::risk(self.as_ref(), name)
    }

    async fn execute(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        shell_name: &str,
        call: ShellCall,
        input: JsonValue,
    ) -> Result<lorelei_core::ShellResult, LoreleiError> {
        ShellRegistryPg::execute(self.as_ref(), tenant_id, run_id, shell_name, call, input).await
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CritiqueDecision {
    pub accept: bool,
    pub reason: String,
}

fn parse_strict_json<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, LoreleiError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(LoreleiError::validation("invalid JSON: empty response".to_string()));
    }
    serde_json::from_str(trimmed).map_err(|e| {
        let snippet = truncate(trimmed, 240);
        LoreleiError::validation(format!("invalid JSON: {e}; snippet={snippet:?}"))
    })
}

fn parse_json_anywhere<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, LoreleiError> {
    // Fast path: standard single JSON value.
    if let Ok(v) = parse_strict_json::<T>(s) {
        return Ok(v);
    }

    // Best-effort: scan for a JSON value starting at any `{` or `[` and try to parse from there.
    for (i, ch) in s.char_indices() {
        if ch != '{' && ch != '[' {
            continue;
        }
        let sub = &s[i..];
        let value: JsonValue = match serde_json::from_str(sub) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let value = trim_json_object_keys(value);

        if let Ok(t) = serde_json::from_value::<T>(value.clone()) {
            return Ok(t);
        }
        // Common wrapper: {"data":[...]} or {"items":[...]}.
        if let Some(inner) = value.get("data").or_else(|| value.get("items")) {
            if let Ok(t) = serde_json::from_value::<T>(inner.clone()) {
                return Ok(t);
            }
        }
    }

    let trimmed = s.trim();
    let snippet = truncate(trimmed, 240);
    Err(LoreleiError::validation(format!(
        "invalid JSON: could not find expected JSON value in output; snippet={snippet:?}"
    )))
}

fn trim_json_object_keys(v: JsonValue) -> JsonValue {
    match v {
        JsonValue::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.trim().to_string(), trim_json_object_keys(v));
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(arr) => JsonValue::Array(arr.into_iter().map(trim_json_object_keys).collect()),
        other => other,
    }
}

fn parse_plan_loose(s: &str) -> Result<Plan, LoreleiError> {
    // Strict schema (preferred): {"steps":[...]}
    if let Ok(p) = parse_strict_json::<Plan>(s) {
        return Ok(p);
    }

    // Common model mistake: return a single step object as the root.
    if let Ok(step) = parse_strict_json::<PlanStep>(s) {
        return Ok(Plan { steps: vec![step] });
    }

    // Another common mistake: return steps array as the root.
    if let Ok(steps) = parse_strict_json::<Vec<PlanStep>>(s) {
        return Ok(Plan { steps });
    }

    // Last attempt: extract a "steps" field manually.
    let v: JsonValue = parse_strict_json(s)?;
    if let Some(steps_v) = v.get("steps") {
        let steps: Vec<PlanStep> = serde_json::from_value(steps_v.clone()).map_err(|e| {
            LoreleiError::validation(format!("invalid Plan.steps: {e}"))
        })?;
        return Ok(Plan { steps });
    }

    Err(LoreleiError::validation(
        "invalid Plan JSON: expected object with `steps`".to_string(),
    ))
}

fn parse_plan_anywhere(s: &str) -> Result<Plan, LoreleiError> {
    // Try the existing loose parser first (single JSON value cases).
    if let Ok(p) = parse_plan_loose(s) {
        return Ok(p);
    }

    // If output contains leading text, scan for a JSON value starting at any `{` or `[` and try
    // to parse from there.
    for (i, ch) in s.char_indices() {
        if ch != '{' && ch != '[' {
            continue;
        }
        let sub = &s[i..];
        let value: JsonValue = match serde_json::from_str(sub) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Ok(p) = serde_json::from_value::<Plan>(value.clone()) {
            return Ok(p);
        }
        if let Ok(step) = serde_json::from_value::<PlanStep>(value.clone()) {
            return Ok(Plan { steps: vec![step] });
        }
        if let Ok(steps) = serde_json::from_value::<Vec<PlanStep>>(value.clone()) {
            return Ok(Plan { steps });
        }
        if let Some(steps_v) = value.get("steps") {
            if let Ok(steps) = serde_json::from_value::<Vec<PlanStep>>(steps_v.clone()) {
                return Ok(Plan { steps });
            }
        }
    }

    let trimmed = s.trim();
    let snippet = truncate(trimmed, 240);
    Err(LoreleiError::validation(format!(
        "invalid Plan JSON: could not find plan in output; snippet={snippet:?}"
    )))
}

fn join_chunks(chunks: &[SongChunk]) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str(&c.content);
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out = s[..max].to_string();
    out.push_str("…");
    out
}

fn read_prompt(path: &str) -> Result<String, LoreleiError> {
    let p = Path::new(path);
    let resolved = if p.exists() {
        p.to_path_buf()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    };
    let text = std::fs::read_to_string(&resolved).map_err(|e| {
        LoreleiError::validation(format!(
            "failed to read prompt `{}`: {e}",
            resolved.display()
        ))
    })?;
    Ok(text)
}
