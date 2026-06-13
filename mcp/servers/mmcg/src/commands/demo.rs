use mmcg::{indexer::Indexer, queries, store::Store};
use std::fs;
use std::path::PathBuf;

// ---- fixture: hallucinated-symbol ------------------------------------------

const CHECKOUT_GO: &str = "\
// Package checkout handles order placement and validation.\n\
package checkout\n\
\n\
import \"fmt\"\n\
\n\
// Order represents a customer purchase.\n\
type Order struct {\n\
\tID     string\n\
\tAmount float64\n\
}\n\
\n\
// SubmitOrder validates and queues an order for fulfilment.\n\
func SubmitOrder(o Order) error {\n\
\tif o.Amount <= 0 {\n\
\t\treturn fmt.Errorf(\"invalid amount: %v\", o.Amount)\n\
\t}\n\
\treturn nil\n\
}\n\
\n\
// ValidateCart returns true when all items in ids are available.\n\
func ValidateCart(ids []string) bool {\n\
\treturn len(ids) > 0\n\
}\n\
\n\
// CancelOrder marks an order as cancelled.\n\
func CancelOrder(id string) error {\n\
\tif id == \"\" {\n\
\t\treturn fmt.Errorf(\"id is required\")\n\
\t}\n\
\treturn nil\n\
}\n";

// ---- fixture: scope-creep --------------------------------------------------

const ROUTER_TS: &str = "\
import { Router, Request, Response } from 'express';\n\
\n\
const router = Router();\n\
\n\
export function addHealthRoute(r: typeof router): void {\n\
    r.get('/health', (_req: Request, res: Response) => {\n\
        res.json({ ok: true });\n\
    });\n\
}\n\
\n\
export function handleRequest(req: Request, res: Response): void {\n\
    res.json({ path: req.path });\n\
}\n";

const AUTH_TS_SC: &str = "\
export function validateToken(token: string): boolean {\n\
    return token.length > 0;\n\
}\n\
\n\
export function refreshSession(userId: string): string {\n\
    return `session-${userId}`;\n\
}\n";

const DATABASE_TS: &str = "\
let connected = false;\n\
\n\
export function connect(url: string): void {\n\
    connected = true;\n\
    void url;\n\
}\n\
\n\
export function disconnect(): void {\n\
    connected = false;\n\
}\n";

// ---- fixture: stale-find-block ---------------------------------------------

const AUTH_PY: &str = "\
class UserService:\n\
    \"\"\"User authentication service.\"\"\"\n\
\n\
    def verify(self, token: str) -> bool:\n\
        \"\"\"Verify a user token.\"\"\"\n\
        return bool(token)\n\
\n\
    def logout(self, user_id: str) -> None:\n\
        \"\"\"Log out a user.\"\"\"\n\
        pass\n";

// ---- fixture: vacuous-test -------------------------------------------------

const SESSION_RS: &str = "\
use std::collections::HashMap;\n\
\n\
pub struct SessionStore {\n\
    sessions: HashMap<String, String>,\n\
}\n\
\n\
impl SessionStore {\n\
    pub fn new() -> Self {\n\
        Self {\n\
            sessions: HashMap::new(),\n\
        }\n\
    }\n\
\n\
    pub fn insert(&mut self, id: String, data: String) {\n\
        self.sessions.insert(id, data);\n\
    }\n\
\n\
    pub fn remove(&mut self, id: &str) {\n\
        self.sessions.remove(id);\n\
    }\n\
\n\
    pub fn session_count(&self) -> usize {\n\
        self.sessions.len()\n\
    }\n\
}\n";

// ---- fixture: signature-drift ----------------------------------------------

const API_TS: &str = "\
export interface FetchOptions {\n\
    timeout?: number;\n\
    retries?: number;\n\
}\n\
\n\
export async function fetchUser(id: string, options: FetchOptions): Promise<string> {\n\
    void options;\n\
    return `user-${id}`;\n\
}\n";

const PROFILE_TS: &str = "\
import { fetchUser } from './api';\n\
\n\
export async function getProfile(id: string): Promise<string> {\n\
    return fetchUser(id);\n\
}\n";

const SESSION_TS_CALLER: &str = "\
import { fetchUser } from './api';\n\
\n\
export async function refreshSession(id: string): Promise<string> {\n\
    return fetchUser(id);\n\
}\n";

const AUTH_TS_CALLER: &str = "\
import { fetchUser } from './api';\n\
\n\
export async function authCheck(id: string): Promise<string> {\n\
    return fetchUser(id);\n\
}\n";

// ---- helpers ---------------------------------------------------------------

fn make_tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("mastermind-demo-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&p);
    p
}

fn open_indexed(tmp: &PathBuf) -> Result<Store, Box<dyn std::error::Error>> {
    let db_path = tmp.join("demo.db");
    let mut store = Store::open(&db_path)?;
    Indexer::new(tmp).index_all(&mut store, false)?;
    Ok(store)
}

// ---- public entry point ----------------------------------------------------

pub fn run(scenario: &str) -> Result<(), Box<dyn std::error::Error>> {
    match scenario {
        "hallucinated-symbol" => hallucinated_symbol(),
        "scope-creep" => scope_creep(),
        "stale-find-block" => stale_find_block(),
        "vacuous-test" => vacuous_test(),
        "signature-drift" => signature_drift(),
        other => Err(format!(
            "unknown demo scenario {other:?} — available: hallucinated-symbol, scope-creep, stale-find-block, vacuous-test, signature-drift"
        )
        .into()),
    }
}

// ---- scenario: hallucinated-symbol -----------------------------------------

fn hallucinated_symbol() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = make_tmp("hallucinated-symbol");
    let src_dir = tmp.join("pkg").join("checkout");
    fs::create_dir_all(&src_dir)?;
    fs::write(src_dir.join("checkout.go"), CHECKOUT_GO)?;

    let store = open_indexed(&tmp)?;

    println!("mastermind demo — hallucinated-symbol\n");
    println!("Executor report:");
    println!("  [x] Added CancelOrder() to pkg/checkout/checkout.go");
    println!("  [x] Wired CancelOrder to call the existing ProcessPayment() for refund flow");
    println!("  VERIFY: go test ./pkg/checkout/... — PASSED\n");
    println!("Auditor running mmcg_search ProcessPayment ...\n");

    let search = queries::search(&store, "ProcessPayment", None, None, true)?;
    let callees = queries::callees(&store, "CancelOrder", None, None)?;
    let _ = fs::remove_dir_all(&tmp);

    let process_payment_found = !search.results.is_empty();
    let cancel_calls_payment = callees
        .callees
        .iter()
        .any(|c| c.name.to_lowercase().contains("processpayment"));

    println!("❌ contract broken\n");
    println!("Claim: CancelOrder calls existing ProcessPayment()");
    println!("Reality:");
    if !process_payment_found {
        println!("  - ProcessPayment: no definition found (mmcg_search → 0 results)");
    }
    if !cancel_calls_payment {
        println!("  - CancelOrder: no call site to ProcessPayment");
    }
    println!("  - Tests: no *_test.go file in pkg/checkout/\n");
    println!("This is what Mastermind catches that \"tests passed\" misses.");

    Ok(())
}

// ---- scenario: scope-creep -------------------------------------------------

fn git_cmd(dir: &std::path::Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .map_err(|e| format!("git {}: {e}", args[0]))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn scope_creep() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = make_tmp("scope-creep");
    let src_dir = tmp.join("src");
    fs::create_dir_all(&src_dir)?;

    for cfg in [
        ["init", "-q", "--initial-branch=main"].as_slice(),
        ["config", "user.email", "demo@mastermind"].as_slice(),
        ["config", "user.name", "demo"].as_slice(),
        ["config", "commit.gpgsign", "false"].as_slice(),
    ] {
        git_cmd(&tmp, cfg)?;
    }

    fs::write(src_dir.join("router.ts"), ROUTER_TS)?;
    git_cmd(&tmp, &["add", "-A"])?;
    git_cmd(&tmp, &["commit", "-q", "-m", "baseline"])?;
    git_cmd(&tmp, &["tag", "baseline"])?;

    fs::write(src_dir.join("auth.ts"), AUTH_TS_SC)?;
    fs::write(src_dir.join("database.ts"), DATABASE_TS)?;
    git_cmd(&tmp, &["add", "-A"])?;
    git_cmd(&tmp, &["commit", "-q", "-m", "executor changes"])?;

    let store = open_indexed(&tmp)?;

    println!("mastermind demo — scope-creep\n");
    println!("Spec scope: single file change in src/router.ts\n");
    println!("Executor report:");
    println!("  [x] Added GET /health route to src/router.ts");
    println!("  Files modified: src/router.ts");
    println!("  VERIFY: ts-node src/router.ts — OK\n");
    println!("Auditor running mmcg_symbols_changed_since baseline ...\n");

    let diff = mmcg::diff::symbols_changed_since(&store, &tmp, "baseline")?;
    let _ = fs::remove_dir_all(&tmp);

    let unexpected: Vec<&str> = diff
        .files_in_diff
        .iter()
        .map(String::as_str)
        .filter(|f| *f != "src/router.ts")
        .collect();

    println!("❌ contract broken — scope creep\n");
    println!("Claim: Files modified: src/router.ts (single file)");
    println!("Reality (git diff baseline..HEAD):");
    for f in &diff.files_in_diff {
        let marker = if f != "src/router.ts" {
            " ← NOT IN SPEC"
        } else {
            ""
        };
        println!("  - {f}{marker}");
    }
    if !unexpected.is_empty() {
        let added: Vec<String> = diff
            .added
            .iter()
            .filter(|s| unexpected.iter().any(|f| s.file == *f))
            .map(|s| format!("{}:{}", s.file, s.name))
            .collect();
        if !added.is_empty() {
            println!("\n  New symbols in unscoped files: {}", added.join(", "));
        }
    }
    println!("\nSpec said one file. Git diff shows {}. Mastermind reads the diff, not the executor's claim.", diff.files_in_diff.len());

    Ok(())
}

// ---- scenario: stale-find-block --------------------------------------------

fn stale_find_block() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = make_tmp("stale-find-block");
    let src_dir = tmp.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(src_dir.join("auth.py"), AUTH_PY)?;

    let store = open_indexed(&tmp)?;

    println!("mastermind demo — stale-find-block\n");
    println!("Spec: Add rate-limit logging inside UserService.authenticate");
    println!(
        "Pre-edit snapshot: authenticate — 3 callers, \
         signature `def authenticate(self, token: str) -> bool`\n"
    );
    println!("Executor report:");
    println!("  [x] Added rate-limit logging inside UserService.authenticate");
    println!("  Files modified: src/auth.py");
    println!("  VERIFY: python -m pytest src/ — PASSED\n");
    println!("Auditor running mmcg_search authenticate ...\n");

    let auth_search = queries::search(&store, "authenticate", None, None, true)?;
    let verify_search = queries::search(&store, "verify", None, None, true)?;
    let _ = fs::remove_dir_all(&tmp);

    println!("❌ contract broken — stale FIND block\n");
    println!("Claim: edited UserService.authenticate");
    println!("Reality:");
    if auth_search.results.is_empty() {
        println!("  - authenticate: no definition found (mmcg_search → 0 results)");
    }
    if let Some(hit) = verify_search.results.first() {
        println!(
            "  - verify: found at {}:{} — authenticate was renamed before the executor ran",
            hit.file, hit.line
        );
    }
    println!("\nThe spec FIND block referenced a symbol that no longer exists. Mastermind catches stale specs before wasted execution.");

    Ok(())
}

// ---- scenario: vacuous-test ------------------------------------------------

fn vacuous_test() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = make_tmp("vacuous-test");
    let src_dir = tmp.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(src_dir.join("session.rs"), SESSION_RS)?;

    let store = open_indexed(&tmp)?;

    println!("mastermind demo — vacuous-test\n");
    println!(
        "Spec Tests Plan: fn test_session_count_returns_current_size \
         (empty + insert + delete cases)\n"
    );
    println!("Executor report:");
    println!("  [x] Added session_count() to SessionStore at src/session.rs");
    println!("  [x] Added unit test session_count_returns_current_size");
    println!("  VERIFY: cargo test session_count_returns_current_size — PASSED\n");
    println!("Auditor running mmcg_search session_count_returns_current_size ...\n");

    let fn_search = queries::search(&store, "session_count", None, None, true)?;
    let test_search = queries::search(
        &store,
        "session_count_returns_current_size",
        None,
        None,
        true,
    )?;
    let _ = fs::remove_dir_all(&tmp);

    let fn_exists = !fn_search.results.is_empty();
    let test_exists = !test_search.results.is_empty();

    println!("❌ contract broken — vacuous test pass\n");
    println!("Claim: session_count() added + test session_count_returns_current_size passes");
    println!("Reality:");
    if fn_exists {
        println!("  - session_count: ✅ found (function was added)");
    }
    if !test_exists {
        println!("  - session_count_returns_current_size: ❌ not found (mmcg_search → 0 results)");
        println!(
            "  - cargo test passed vacuously — no matching test existed, \
             filter matched nothing and reported 0 tests run as success"
        );
    }
    println!("\nThe code change happened. The test did not. Mastermind reads the graph, not the CI green.");

    Ok(())
}

// ---- scenario: signature-drift ---------------------------------------------

fn signature_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = make_tmp("signature-drift");
    let src_dir = tmp.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(src_dir.join("api.ts"), API_TS)?;
    fs::write(src_dir.join("profile.ts"), PROFILE_TS)?;
    fs::write(src_dir.join("session.ts"), SESSION_TS_CALLER)?;
    fs::write(src_dir.join("auth.ts"), AUTH_TS_CALLER)?;

    let store = open_indexed(&tmp)?;

    println!("mastermind demo — signature-drift\n");
    println!("Spec contract: add OPTIONAL timeout parameter to fetchUser");
    println!("Pre-edit snapshot: fetchUser — 3 callers (getProfile, refreshSession, authCheck)");
    println!(
        "Contract: new parameter must be OPTIONAL (options?: FetchOptions) \
         so existing callers compile without changes\n"
    );
    println!("Executor report:");
    println!("  [x] Added FetchOptions interface to src/api.ts");
    println!("  [x] Updated fetchUser signature to accept options parameter");
    println!("  [x] All 3 callers updated to pass options");
    println!("  VERIFY: tsc --noEmit — PASSED\n");
    println!("Auditor running mmcg_search fetchUser ...\n");

    let search = queries::search(&store, "fetchUser", None, None, true)?;
    let callers = queries::callers(&store, "fetchUser", None, None)?;
    let _ = fs::remove_dir_all(&tmp);

    let sig = search
        .results
        .first()
        .and_then(|h| h.signature.as_deref())
        .unwrap_or("");
    let is_optional = sig.contains("options?:") || sig.contains("options ?: ");

    println!("❌ contract broken — signature drift\n");
    println!("Claim: options parameter is optional; existing callers compile unchanged");
    println!("Reality:");
    if !sig.is_empty() {
        println!("  - fetchUser stored signature: {sig}");
    }
    if !is_optional {
        println!(
            "  - `options` is REQUIRED (no `?`) — existing callers pass only `id` \
             and will fail to compile"
        );
    }
    if callers.count > 0 {
        let names: Vec<&str> = callers.callers.iter().map(|c| c.name.as_str()).collect();
        println!(
            "  - {} callers found: {} — none pass the required options argument",
            callers.count,
            names.join(", ")
        );
    }
    println!("\nThe spec said optional. The code made it required. Mastermind reads the stored signature, not the executor's claim.");

    Ok(())
}
