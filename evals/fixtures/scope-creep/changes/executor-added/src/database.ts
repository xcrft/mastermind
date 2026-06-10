// Database connection — scope-creep variant.
// UNRELATED TO SPEC — executor changed connection pool defaults here.
// Spec scoped changes to src/router.ts only.

export interface DbConfig {
  host: string;
  port: number;
  name: string;
  poolSize?: number;
}

const DEFAULT_POOL_SIZE = 10;

export function connect(cfg: DbConfig): Promise<void> {
  const pool = cfg.poolSize ?? DEFAULT_POOL_SIZE;
  console.log(
    `connecting to ${cfg.host}:${cfg.port}/${cfg.name} (pool=${pool})`
  );
  return Promise.resolve();
}
