use super::*;
use std::sync::Weak;

fn cloud_account(key: &str, base: &str) -> crate::managed_cloud::Account {
    crate::managed_cloud::Account {
        api_key: key.into(),
        api_base: base.into(),
    }
}

#[test]
fn managed_cloud_reuses_only_exact_authenticated_account() {
    let mut cloud = CloudTransport::<_, ()>::default();
    let account = cloud_account("first", "https://api.example/v1");
    cloud.select_account(Some(&account), 1);
    let owner = Arc::new(1);
    cloud.publish(Arc::downgrade(&owner));
    cloud.select_account(Some(&account), 1);
    assert!(Arc::ptr_eq(&owner, &cloud.reserve(Weak::upgrade).unwrap()));
    for changed in [
        cloud_account("second", "https://api.example/v1"),
        cloud_account("second", "https://other.example/v1"),
    ] {
        cloud.select_account(Some(&changed), 1);
        assert!(cloud.reserve(Weak::upgrade).is_none());
        cloud.publish(Arc::downgrade(&owner));
    }
    cloud.select_account(None, 1);
    assert!(cloud.reserve(Weak::upgrade).is_none());
    // Signing back in must not resurrect a master from before sign-out.
    cloud.select_account(Some(&account), 1);
    assert!(cloud.reserve(Weak::upgrade).is_none());
    assert_eq!(*owner, 1);
}

#[test]
fn managed_cloud_changed_parity_fingerprint_invalidates_live_master() {
    let mut cloud = CloudTransport::<_, ()>::default();
    let account = cloud_account("first", "https://api.example/v1");
    let owner = Arc::new(1);
    cloud.select_account(Some(&account), 10);
    cloud.publish(Arc::downgrade(&owner));
    cloud.select_account(Some(&account), 10);
    assert!(cloud.reserve(Weak::upgrade).is_some());
    cloud.select_account(Some(&account), 11);
    assert!(cloud.reserve(Weak::upgrade).is_none());
    assert_eq!(cloud.reservations, 0);
    cloud.publish(Arc::downgrade(&owner));
    cloud.select_account(Some(&account), 10);
    assert!(cloud.reserve(Weak::upgrade).is_none());
    // Existing clients remain alive even when defaults change.
    assert_eq!(*owner, 1);
}

#[test]
fn managed_cloud_reservations_are_bounded_and_new_master_resets_budget() {
    let mut cloud = CloudTransport::<_, ()>::default();
    let owner = Arc::new(1);
    cloud.publish(Arc::downgrade(&owner));
    for _ in 1..CHANNELS_PER_MASTER {
        assert!(Arc::ptr_eq(&owner, &cloud.reserve(Weak::upgrade).unwrap()));
    }
    assert!(cloud.reserve(Weak::upgrade).is_none());
    assert!(cloud.owner.is_none());
    let replacement = Arc::new(2);
    cloud.publish(Arc::downgrade(&replacement));
    assert_eq!(cloud.reservations, 1);
    assert!(Arc::ptr_eq(
        &replacement,
        &cloud.reserve(Weak::upgrade).unwrap()
    ));
    assert_eq!(*owner, 1);
}

#[test]
fn managed_cloud_failure_and_expired_owner_require_fresh_initialization() {
    let mut cloud = CloudTransport::<_, ()>::default();
    let owner = Arc::new(1);
    cloud.publish(Arc::downgrade(&owner));
    cloud.invalidate();
    assert!(cloud.reserve(Weak::upgrade).is_none());
    cloud.publish(Arc::downgrade(&owner));
    let weak = Arc::downgrade(&owner);
    drop(owner);
    assert!(weak.upgrade().is_none());
    assert!(cloud.reserve(Weak::upgrade).is_none());
    assert!(cloud.owner.is_none());
}

#[test]
fn cloud_initialization_does_not_lock_ordinary_ssh_pool() {
    let transports = RemoteTransports::default();
    let _cloud_initialization = transports.cloud.lock().unwrap();
    assert!(transports.pool.try_lock().is_ok());
    assert!(Arc::ptr_eq(&transports.cloud, &transports.clone().cloud));
    assert!(!Arc::ptr_eq(
        &transports.cloud,
        &RemoteTransports::default().cloud
    ));
}

fn spare_cache() -> (CloudTransport<Weak<usize>, Arc<usize>>, Arc<usize>) {
    let owner = Arc::new(1);
    let mut cloud = CloudTransport::default();
    cloud.select_account(Some(&cloud_account("first", "https://api.example/v1")), 10);
    cloud.publish(Arc::downgrade(&owner));
    (cloud, owner)
}

#[test]
fn cloud_spare_is_one_shot_and_reserves_only_during_preparation() {
    let (mut cloud, owner) = spare_cache();
    let now = Instant::now();
    let (generation, prepared_owner) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    assert!(Arc::ptr_eq(&owner, &prepared_owner));
    assert_eq!(cloud.reservations, 2);
    assert!(cloud.begin_prepare(Weak::upgrade, now).is_none());
    assert!(cloud.finish_prepare(generation, Some(prepared_owner), now));
    assert!(cloud.begin_prepare(Weak::upgrade, now).is_none());
    assert!(Arc::ptr_eq(&owner, &cloud.take_spare(now).unwrap()));
    assert_eq!(cloud.reservations, 2);
    assert!(cloud.take_spare(now).is_none());
    let (next, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    assert_ne!(generation, next);
    assert_eq!(cloud.reservations, 3);
}

#[test]
fn cloud_spare_expiration_actively_drops_last_connection() {
    let (mut cloud, owner) = spare_cache();
    let weak = Arc::downgrade(&owner);
    let now = Instant::now();
    let (generation, prepared_owner) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    assert!(cloud.finish_prepare(generation, Some(prepared_owner), now));
    drop(owner);
    cloud.expire_spare(generation, now + SPARE_TTL - Duration::from_nanos(1));
    assert!(weak.upgrade().is_some());
    cloud.expire_spare(generation, now + SPARE_TTL);
    assert!(weak.upgrade().is_none());
    assert!(cloud.take_spare(now + SPARE_TTL).is_none());
}

#[test]
fn cloud_spare_take_enforces_deadline_even_before_timer_runs() {
    let (mut cloud, owner) = spare_cache();
    let now = Instant::now();
    let (generation, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    cloud.finish_prepare(generation, Some(owner), now);
    assert!(cloud.take_spare(now + SPARE_TTL).is_none());
}

#[test]
fn cloud_spare_account_and_fingerprint_changes_drop_ready_and_fence_pending() {
    for (account, fingerprint) in [
        (Some(cloud_account("second", "https://api.example/v1")), 10),
        (Some(cloud_account("first", "https://other.example/v1")), 10),
        (Some(cloud_account("first", "https://api.example/v1")), 11),
        (None, 10),
    ] {
        let (mut cloud, owner) = spare_cache();
        let now = Instant::now();
        let (generation, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
        cloud.select_account(account.as_ref(), fingerprint);
        assert!(!cloud.finish_prepare(generation, Some(owner.clone()), now));
        assert!(cloud.take_spare(now).is_none());
        assert!(cloud.owner.is_none());

        let (mut cloud, owner) = spare_cache();
        let (generation, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
        cloud.finish_prepare(generation, Some(owner), now);
        cloud.select_account(account.as_ref(), fingerprint);
        assert!(cloud.take_spare(now).is_none());
    }
}

#[test]
fn cloud_spare_stale_completion_and_timer_cannot_touch_replacement() {
    let (mut cloud, owner) = spare_cache();
    let now = Instant::now();
    let (old, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    cloud.publish(Arc::downgrade(&owner));
    let (new, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    assert!(!cloud.finish_prepare(old, Some(owner.clone()), now));
    assert!(cloud.preparing);
    assert!(cloud.finish_prepare(new, Some(owner), now + SPARE_TTL));
    cloud.expire_spare(old, now + SPARE_TTL * 3);
    assert!(cloud.take_spare(now + SPARE_TTL).is_some());
}

#[test]
fn cloud_spare_failed_background_does_not_retry_or_refund_budget() {
    let (mut cloud, _owner) = spare_cache();
    let now = Instant::now();
    let (generation, _) = cloud.begin_prepare(Weak::upgrade, now).unwrap();
    assert!(!cloud.finish_prepare(generation, None, now));
    assert!(!cloud.preparing);
    assert!(cloud.spare.is_none());
    assert_eq!(cloud.reservations, 2);
    assert!(!cloud.finish_prepare(generation, Some(Arc::new(9)), now));
    assert!(cloud.take_spare(now).is_none());
}

fn target(host: &str, identity: u64) -> Target {
    Target {
        host: host.into(),
        identity,
    }
}

fn acquire(pool: &mut Pool<Weak<usize>>, key: Target, value: usize) -> (Arc<usize>, u64) {
    pool.acquire(key, Weak::upgrade, || {
        let owner = Arc::new(value);
        let weak = Arc::downgrade(&owner);
        Ok::<_, ()>((owner, weak))
    })
    .unwrap()
}

#[test]
fn same_target_shares_owner_but_not_other_hosts_or_other_bridge_runs() {
    let mut pool = Pool::default();
    let (one, _) = acquire(&mut pool, target("alpha", 1), 1);
    let (two, _) = acquire(&mut pool, target("alpha", 1), 2);
    let (other, _) = acquire(&mut pool, target("beta", 1), 3);
    let (other_run, _) = acquire(&mut Pool::default(), target("alpha", 1), 4);
    assert!(Arc::ptr_eq(&one, &two));
    assert!(!Arc::ptr_eq(&one, &other));
    assert!(!Arc::ptr_eq(&one, &other_run));
}

#[test]
fn weak_cache_never_keeps_idle_master_alive() {
    let mut pool = Pool::default();
    let (owner, _) = acquire(&mut pool, target("alpha", 1), 1);
    let weak = Arc::downgrade(&owner);
    drop(owner);
    assert!(weak.upgrade().is_none());
    let (replacement, _) = acquire(&mut pool, target("alpha", 1), 2);
    assert_eq!(*replacement, 2);
}

#[test]
fn retarget_and_eviction_preserve_live_old_clients() {
    let mut pool = Pool::default();
    let (old, old_generation) = acquire(&mut pool, target("alias", 1), 1);
    let (new, _) = acquire(&mut pool, target("alias", 2), 2);
    assert_eq!(pool.entries.len(), 1);
    pool.remove(&target("alias", 1), old_generation);
    assert_eq!(pool.entries.len(), 1);
    assert_eq!((*old, *new), (1, 2));
    let mut live = vec![old, new];
    for index in 0..MAX_TARGETS + 10 {
        live.push(acquire(&mut pool, target(&format!("host-{index}"), 0), index).0);
        assert!(pool.entries.len() <= MAX_TARGETS);
    }
    assert_eq!(*live[0], 1);
}

#[test]
fn late_failure_cannot_evict_replacement_generation() {
    let mut pool = Pool::default();
    let key = target("alpha", 1);
    let (old, generation) = acquire(&mut pool, key.clone(), 1);
    pool.remove(&key, generation);
    let (replacement, next_generation) = acquire(&mut pool, key.clone(), 2);
    pool.remove(&key, generation);
    assert_eq!(pool.entries.len(), 1);
    assert_ne!(generation, next_generation);
    assert_eq!((*old, *replacement), (1, 2));
}

#[test]
fn concurrent_reservations_shard_before_default_sshd_channel_limit() {
    let pool = Arc::new(Mutex::new(Pool::default()));
    let barrier = Arc::new(std::sync::Barrier::new(24));
    let workers: Vec<_> = (0..24)
        .map(|index| {
            let pool = pool.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let (owner, generation) =
                    acquire(&mut pool.lock().unwrap(), target("alpha", 1), index);
                barrier.wait(); // keep all 24 leases simultaneously live
                (owner, generation)
            })
        })
        .collect();
    let leases: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    let mut counts = std::collections::HashMap::new();
    for (_, generation) in &leases {
        *counts.entry(*generation).or_insert(0) += 1;
    }
    assert_eq!(counts.len(), 3);
    assert!(counts.values().all(|&count| count == CHANNELS_PER_MASTER));
}

#[test]
fn config_identity_changes_for_alias_port_proxy_and_identity_file_replacement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("key with spaces");
    std::fs::write(&path, "first").unwrap();
    let config = format!(
        "hostname first\nport 22\nuser user\nidentityfile {}\n",
        path.display()
    );
    let baseline = fingerprint("alias", &config).unwrap();
    assert_eq!(fingerprint("alias", &config), Some(baseline));
    assert_ne!(
        fingerprint("alias", &config.replace("first", "second")),
        Some(baseline)
    );
    assert_ne!(
        fingerprint("alias", &config.replace("22", "2222")),
        Some(baseline)
    );
    assert_ne!(
        fingerprint("alias", &(config.clone() + "proxycommand changed\n")),
        Some(baseline)
    );
    std::fs::write(&path, "replacement key material").unwrap();
    assert_ne!(fingerprint("alias", &config), Some(baseline));
}

#[test]
fn tokenized_identity_and_spaced_known_hosts_changes_are_observed() {
    let root = tempfile::tempdir().unwrap();
    let key = root.path().join("key_server_user");
    let known = root.path().join("known hosts");
    std::fs::write(&key, "key").unwrap();
    std::fs::write(&known, "known").unwrap();
    let config = format!(
        "hostname server\nuser user\nidentityfile {}/key_%h_%r\nuserknownhostsfile {}\n",
        root.path().display(),
        known.display()
    );
    let before = fingerprint("alias", &config);
    std::fs::write(&key, "new key").unwrap();
    let after_key = fingerprint("alias", &config);
    assert_ne!(before, after_key);
    std::fs::write(&known, "new host").unwrap();
    assert_ne!(after_key, fingerprint("alias", &config));
    // Unknown expansions use isolated SSH instead of unsafe cache identity.
    assert_eq!(fingerprint("alias", "identityfile /keys/%C\n"), None);
    assert_eq!(fingerprint("alias", "identityfile ${CUSTOM_KEY}\n"), None);
}

#[test]
fn cloud_helper_retargeting_invalidates_unchanged_ssh_config() {
    let root = tempfile::tempdir().unwrap();
    let home = Some(root.path().as_os_str().to_owned());
    std::fs::create_dir_all(root.path().join(".config/jcode")).unwrap();
    std::fs::create_dir_all(root.path().join(".local/bin")).unwrap();
    let config_path = root.path().join(".config/jcode/cloud-alpha.json");
    let helper = root.path().join(".local/bin/jcode-cloud-alpha");
    std::fs::write(&config_path, "instance-a").unwrap();
    std::fs::write(&helper, "helper-v1").unwrap();
    let identity = || {
        fingerprint_with_environment(
            "jcode-cloud-alpha",
            "hostname unchanged\n",
            home.clone(),
            None,
        )
    };
    let before = identity();
    std::fs::write(&config_path, "instance-b-longer").unwrap();
    let retargeted = identity();
    assert_ne!(before, retargeted);
    std::fs::write(&helper, "helper-v2-longer").unwrap();
    assert_ne!(retargeted, identity());
}
