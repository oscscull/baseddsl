import assert from "node:assert/strict";
import { post } from "./request.js";

export async function verifyRequiredContext(privateUrl: string): Promise<void> {
  const missing = await post(`${privateUrl}/q/my_orders`, {});
  assert.equal(missing.status, 400);
  assert.equal(missing.body.error.code, "missing_ctx");
}
