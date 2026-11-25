//! OIDC email-linking rules of `Store::upsert_oidc_user` (issue #192).

use std::sync::Arc;

use base64::Engine;
use orbit_domain::{OrganizationMode, UserRole};
use orbit_security::SecretCipher;
use orbit_store::{Store, StoreError};
use uuid::Uuid;

async fn test_store() -> Option<Store> {
    let url = orbit_store::test_database_url()?;
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let cipher = Arc::new(SecretCipher::from_base64_key(&key).unwrap());
    let store = Store::connect(&url, 5)
        .await
        .expect("connect")
        .with_cipher(cipher);
    store.migrate().await.expect("migrate");
    Some(store)
}

/// The OIDC identity index is global, so every test uses its own issuer.
fn issuer() -> String {
    format!("https://idp-{}.test", Uuid::now_v7())
}

async fn seed_org(store: &Store) -> Uuid {
    let slug = format!("oidc-{}", Uuid::now_v7());
    store
        .create_organization(&slug, "OIDC Org", OrganizationMode::Personal)
        .await
        .expect("org")
        .id
}

#[tokio::test]
async fn second_subject_with_same_email_is_refused_and_user_unchanged() {
    let Some(store) = test_store().await else {
        return;
    };
    let org = seed_org(&store).await;
    let issuer = issuer();
    let issuer = issuer.as_str();
    let first = store
        .upsert_oidc_user(
            org,
            issuer,
            "sub-1",
            Some("alice@test.local"),
            None,
            UserRole::Viewer,
            true,
        )
        .await
        .expect("first login");
    let err = store
        .upsert_oidc_user(
            org,
            issuer,
            "sub-2",
            Some("Alice@test.local"),
            None,
            UserRole::Viewer,
            true,
        )
        .await
        .expect_err("second subject must be refused");
    assert!(matches!(err, StoreError::Conflict(_)), "{err}");

    let unchanged = store
        .get_user_by_email(org, "alice@test.local")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.id, first.id);
    assert_eq!(unchanged.oidc_subject.as_deref(), Some("sub-1"));
    assert!(store
        .get_user_by_oidc(issuer, "sub-2")
        .await
        .unwrap()
        .is_none());

    // The original identity still resolves to the same user.
    let again = store
        .upsert_oidc_user(org, issuer, "sub-1", None, None, UserRole::Viewer, true)
        .await
        .unwrap();
    assert_eq!(again.id, first.id);
}

#[tokio::test]
async fn invited_user_is_linked_on_first_login() {
    let Some(store) = test_store().await else {
        return;
    };
    let org = seed_org(&store).await;
    let issuer = issuer();
    let issuer = issuer.as_str();
    let invited = store
        .ensure_user(org, "invitee@test.local", "Invitee", UserRole::Operator)
        .await
        .unwrap();
    assert!(invited.oidc_subject.is_none());
    let linked = store
        .upsert_oidc_user(
            org,
            issuer,
            "sub-invitee",
            Some("invitee@test.local"),
            None,
            UserRole::Viewer,
            true,
        )
        .await
        .expect("invited user links");
    assert_eq!(linked.id, invited.id);
    assert_eq!(linked.oidc_subject.as_deref(), Some("sub-invitee"));
    assert_eq!(linked.role, "operator");
}

#[tokio::test]
async fn disallowed_email_link_does_not_bind_invited_user() {
    let Some(store) = test_store().await else {
        return;
    };
    let org = seed_org(&store).await;
    let issuer = issuer();
    let issuer = issuer.as_str();
    let invited = store
        .ensure_user(org, "pending@test.local", "Pending", UserRole::Admin)
        .await
        .unwrap();
    let err = store
        .upsert_oidc_user(
            org,
            issuer,
            "sub-attacker",
            Some("pending@test.local"),
            None,
            UserRole::Viewer,
            false,
        )
        .await
        .expect_err("unverified email must not link");
    assert!(matches!(err, StoreError::Conflict(_)), "{err}");
    let after = store.get_user(org, invited.id).await.unwrap();
    assert!(after.oidc_subject.is_none());
}

#[tokio::test]
async fn concurrent_first_login_resolves_to_the_same_user() {
    let Some(store) = test_store().await else {
        return;
    };
    let org = seed_org(&store).await;
    let issuer = issuer();
    let issuer = issuer.as_str();
    let login = |store: Store| async move {
        store
            .upsert_oidc_user(
                org,
                issuer,
                "sub-race",
                Some("race@test.local"),
                None,
                UserRole::Viewer,
                true,
            )
            .await
    };
    let (a, b) = tokio::join!(login(store.clone()), login(store.clone()));
    assert_eq!(a.expect("first").id, b.expect("second").id);
}
