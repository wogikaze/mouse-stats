import { defineScheduled } from "void";
import { cleanupExpired } from "../src/api";

export const cron = "*/15 * * * *";

export default defineScheduled(async (_controller, env) => {
  await cleanupExpired(env.DB);
});
