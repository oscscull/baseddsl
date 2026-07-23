# `tx` step bindings (D107): `create … as name` binds a step's produced row; a later
# step references a column of it as `$name.field`, reaching ANY prior step. Here step 3
# (Log) references both step 1 (org) and step 2 (user) — the case `^` could not express.
Org { id: Id, name: text }
User { id: Id, org: Org, email: text }
Log { id: Id, org: Org, actor: User }

shape OrgCard from Org { name }

mutation onboard(name: text, email: text) -> OrgCard {
  tx {
    create Org { name = $name } as org;
    create User { org = $org.id, email = $email } as user;
    create Log { org = $org.id, actor = $user.id };
  }
}
