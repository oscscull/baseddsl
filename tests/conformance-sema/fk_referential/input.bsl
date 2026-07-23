# FK referential actions: the convention-free structural checks.
# (The divergence-reason rule E0295/W0110 is a manifest-dependent pass, unit-tested.)
Org { id: Id  name: text }

Order {
  id: Id
  org:  Org @fk(on_delete: cascade)
  note: text @fk                          # E0290 — @fk on a scalar
  owner: Org? @fk(on_delete: set_null)    # clean: nullable, so set_null is allowed
  both: Org @fk @no_fk                    # E0292 — opted in and out
  hard: Org @fk(on_delete: set_null)      # E0293 — set_null on a required relation
}
