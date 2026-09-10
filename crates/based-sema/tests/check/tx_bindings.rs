use super::*;

// ---------- tx step bindings (`create … as name` / `$name.field`) -----------

#[test]
fn tx_step_ref_to_prior_create_is_clean() {
    assert_clean(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#,
    );
}

#[test]
fn tx_step_ref_reaches_any_prior_step_is_clean() {
    // A 3-step tx where step 3 references step 1 — the case `^` could not express.
    assert_clean(
        r#"
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
        "#,
    );
}

#[test]
fn tx_step_ref_to_unknown_field_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.nope, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0111"), "{:?}", codes(&d));
}

#[test]
fn tx_step_binding_shadowing_a_param_rejected() {
    // A binding may not shadow a param — `$user` must name one thing (E0280).
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(user: text, city: text) -> UserCard {
          tx {
            create User { email = $user } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0280"), "{:?}", codes(&d));
}

#[test]
fn tx_duplicate_step_binding_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        shape UserCard from User { email }
        mutation twins(a: text, b: text) -> UserCard {
          tx {
            create User { email = $a } as u;
            create User { email = $b } as u;
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0280"), "{:?}", codes(&d));
}

#[test]
fn tx_forward_reference_rejected() {
    // `$later` is bound by a *later* step — a binding reaches only prior steps (E0281).
    let (_, d) = analyze(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape A from Address { city }
        mutation m(email: text, city: text) -> A {
          tx {
            create Address { user = $later.id, city = $city };
            create User { email = $email } as later;
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0281"), "{:?}", codes(&d));
}

#[test]
fn tx_unbound_name_rejected() {
    // `$nope` is neither a param nor a bound step (E0281).
    let (_, d) = analyze(
        r#"
        Address { id: Id, city: text, ref_id: text }
        shape A from Address { city }
        mutation m(city: text) -> A {
          tx {
            create Address { ref_id = $nope.id, city = $city };
          }
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0281"), "{:?}", codes(&d));
}

#[test]
fn step_ref_outside_tx_rejected() {
    // A step reference in a plain (non-tx) create has no binding in scope (E0281).
    let (_, d) = analyze(
        r#"
        Address { id: Id, city: text, ref_id: text }
        shape A from Address { city }
        mutation m(city: text) -> A {
          create Address { ref_id = $x.id, city = $city };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0281"), "{:?}", codes(&d));
}
