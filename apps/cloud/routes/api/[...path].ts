import { defineHandler } from "void";
import api, { type Env } from "../../src/api";
const handle = defineHandler((c) => api.fetch(c.req.raw, c.env as unknown as Env));
export const GET = handle;
export const POST = handle;
export const PATCH = handle;
export const DELETE = handle;
