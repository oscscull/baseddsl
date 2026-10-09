import assert from "node:assert/strict";
import { post } from "./request.js";

export async function verifyPermissions(url: string, org: string, callback: string) {
  assert.equal((await post(`${callback}/check`, { version: 1 }, { Authorization: "Bearer forged" })).status, 401);
  assert.equal((await post(`${url}/m/place_order`, { total: 7 })).status, 401);
  const buyer = { Authorization: "Bearer local-buyer-token", "Idempotency-Key": "demo-order" };
  const allowed = await post(`${url}/m/place_order`, { total: 7 }, buyer);
  assert.equal(allowed.status, 200);
  assert.deepEqual(await post(`${url}/m/place_order`, { total: 7 }, buyer), allowed);
  const denied = await post(`${url}/m/place_order`, { total: 7 }, {
    Authorization: "Bearer local-viewer-token", "X-Based-Context": JSON.stringify({ org, user: "buyer" }), "X-Based-Shard-Key": "forged"
  });
  assert.equal(denied.status, 403);
  assert.equal(denied.body.error.code, "guard_denied");
  assert.equal((await post(`${url}/m/create_org`, { name: "unauthorized" }, buyer)).status, 404);
  console.log("buyer: allowed (200); viewer with forged context: denied (403)");
}

export async function verifyUnavailable(url: string) {
  const buyer = { Authorization: "Bearer local-buyer-token", "Idempotency-Key": "demo-order" };
  const unavailable = await post(`${url}/m/place_order`, { total: 7 }, buyer);
  assert.equal(unavailable.status, 403);
  assert.equal(unavailable.body.error.message, "Guard check unavailable");
  const orders = await post(`${url}/q/my_orders`, {}, buyer);
  assert.equal(orders.status, 200);
  assert.equal(orders.body.length, 1);
  console.log("forged callback: rejected (401); stopped callback: denied (403); persisted orders: 1");
}
