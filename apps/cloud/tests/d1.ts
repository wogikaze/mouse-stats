import { DatabaseSync } from "node:sqlite";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

// Execute the production migration and SQL verbatim in SQLite; batch is atomic.
export function testDatabase(): D1Database & {
  close(): void;
  execRaw(sql: string): void;
  queryCount(): number;
} {
  let count = 0;
  const sqlite = new DatabaseSync(":memory:");
  sqlite.exec("PRAGMA foreign_keys=ON");
  const dir = fileURLToPath(new URL("../db/migrations/", import.meta.url));
  for (const file of readdirSync(dir)
    .filter((f) => f.endsWith(".sql"))
    .sort())
    sqlite.exec(readFileSync(`${dir}/${file}`, "utf8"));
  function statement(sql: string, args: unknown[] = []): D1PreparedStatement {
    const params = () => args as (string | number | bigint | null | Uint8Array)[];
    return {
      bind: (...values: unknown[]) => statement(sql, values),
      first: async (column?: string) => {
        count++;
        const r = sqlite.prepare(sql).get(...params());
        return r ? (column ? r[column] : r) : null;
      },
      all: async () => (
        count++,
        {
          success: true,
          results: sqlite.prepare(sql).all(...params()),
          meta: { changes: 0, duration: 0 },
        }
      ),
      run: async () => {
        count++;
        const r = sqlite.prepare(sql).run(...params());
        return {
          success: true,
          results: [],
          meta: { changes: Number(r.changes), last_row_id: Number(r.lastInsertRowid), duration: 0 },
        };
      },
      raw: async () =>
        sqlite
          .prepare(sql)
          .all(...params())
          .map((r) => Object.values(r)),
    } as unknown as D1PreparedStatement;
  }
  return {
    prepare: statement,
    batch: async (statements: D1PreparedStatement[]) => {
      sqlite.exec("BEGIN");
      try {
        const result = [];
        for (const st of statements) result.push(await st.run());
        sqlite.exec("COMMIT");
        return result;
      } catch (e) {
        sqlite.exec("ROLLBACK");
        throw e;
      }
    },
    close: () => sqlite.close(),
    queryCount: () => count,
    execRaw: (sql: string) => sqlite.exec(sql),
  } as unknown as D1Database & { close(): void; execRaw(sql: string): void; queryCount(): number };
}
