// Database connection — fixture for scope-creep auditor eval.

export interface DbConfig {
  host: string;
  port: number;
  name: string;
}

export function connect(cfg: DbConfig): Promise<void> {
  console.log(`connecting to ${cfg.host}:${cfg.port}/${cfg.name}`);
  return Promise.resolve();
}
