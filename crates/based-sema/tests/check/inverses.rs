use super::*;

// ---------- inverses -------------------------------------------------------

#[test]
fn inverse_ambiguous() {
    let (_, d) = analyze(
        r#"
        User { id: Id, invites: Membership[] }
        Membership { id: Id, inviter: User, invitee: User }
        "#,
    );
    assert_eq!(errors(&d), ["E0124"]);
}

#[test]
fn inverse_ref_not_forward_edge() {
    let (_, d) = analyze(
        r#"
        User { id: Id, posts: Post[] (Post.nope) }
        Post { id: Id, author: User }
        "#,
    );
    assert_eq!(errors(&d), ["E0123"]);
}
