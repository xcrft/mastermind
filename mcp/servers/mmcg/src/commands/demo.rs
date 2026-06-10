use mmcg::{indexer::Indexer, queries, store::Store};
use std::fs;

const CHECKOUT_GO: &str = "// Package checkout handles order placement and validation.\npackage checkout\n\nimport \"fmt\"\n\n// Order represents a customer purchase.\ntype Order struct {\n\tID     string\n\tAmount float64\n}\n\n// SubmitOrder validates and queues an order for fulfilment.\nfunc SubmitOrder(o Order) error {\n\tif o.Amount <= 0 {\n\t\treturn fmt.Errorf(\"invalid amount: %v\", o.Amount)\n\t}\n\treturn nil\n}\n\n// ValidateCart returns true when all items in ids are available.\nfunc ValidateCart(ids []string) bool {\n\treturn len(ids) > 0\n}\n\n// CancelOrder marks an order as cancelled.\nfunc CancelOrder(id string) error {\n\tif id == \"\" {\n\t\treturn fmt.Errorf(\"id is required\")\n\t}\n\treturn nil\n}\n";

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = std::env::temp_dir().join(format!("mastermind-demo-{}", std::process::id()));
    let src_dir = tmp.join("pkg").join("checkout");
    fs::create_dir_all(&src_dir)?;
    fs::write(src_dir.join("checkout.go"), CHECKOUT_GO)?;

    let db_path = tmp.join("demo.db");
    let mut store = Store::open(&db_path)?;
    Indexer::new(&tmp).index_all(&mut store, false)?;

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
