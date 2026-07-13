# The auth bootstrap. Both callables opt out of the Tenant scope with a written
# reason: a token must resolve — and a login must issue — before any tenant
# context exists to scope by. Every `unscoped` in the app greps to a reason.

query session_by_token(token) -> SessionCtx unscoped("auth: resolves the bearer token before any tenant context exists");

mutation start_session(org: Id, user: Id, token: text) -> SessionCtx unscoped("auth: login issues the session every later context derives from") {
  create Session { org = $org, user = $user, token = $token };
}
