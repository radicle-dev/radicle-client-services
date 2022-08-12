pub mod setup {
    use std::path::Path;
    use std::{env, fs};

    use git2::Oid;

    use librad::crypto::keystore::crypto::{Pwhash, KDF_PARAMS_TEST};
    use librad::crypto::keystore::pinentry::SecUtf8;
    use librad::crypto::BoxedSigner;
    use librad::git::identities::Project;
    use librad::git::util;
    use librad::git_ext::tree;
    use librad::profile::{Profile, LNK_HOME};

    use radicle_common::cobs::patch::MergeTarget;
    use radicle_common::cobs::shared::Store;
    use radicle_common::{keys, person, profile, project, test};

    #[allow(dead_code)]
    pub fn env() -> (Profile, BoxedSigner, Project, Oid) {
        let tempdir = env::temp_dir().join("rad").join("home").join("api");
        let home = env::var(LNK_HOME)
            .map(|s| Path::new(&s).to_path_buf())
            .unwrap_or_else(|_| tempdir.to_path_buf());

        env::set_var(LNK_HOME, home);

        let name = "cloudhead";
        let pass = Pwhash::new(SecUtf8::from(test::USER_PASS), *KDF_PARAMS_TEST);
        let (profile, peer_id) = profile::create(profile::home(), pass.clone()).unwrap();
        let signer = test::signer(&profile, pass).unwrap();
        let storage = keys::storage(&profile, signer.clone()).unwrap();
        let person = person::create(&profile, name, signer.clone(), &storage).unwrap();

        person::set_local(&storage, &person).unwrap();

        let whoami = person::local(&storage).unwrap();
        let payload = project::payload(
            "nakamoto".to_owned(),
            "Bitcoin light-client".to_owned(),
            "master".to_owned(),
        );
        let project = project::create(payload, &storage).unwrap();

        let commit = util::quick_commit(
            &storage,
            &project.urn(),
            vec![
                ("HI", tree::blob(b"Hi Bob")),
                ("README", tree::blob(b"This is a readme")),
            ]
            .into_iter()
            .collect(),
            "say hi to bob",
        )
        .unwrap();

        // create remote head
        // otherwise testing `.../patches/:id` would fail
        fs::create_dir_all(
            profile
                .paths()
                .git_dir()
                .join("refs")
                .join("namespaces")
                .join(&project.urn().encode_id())
                .join("refs")
                .join("remotes")
                .join(peer_id.to_string())
                .join("heads"),
        )
        .unwrap();

        fs::write(
            profile
                .paths()
                .git_dir()
                .join("refs")
                .join("namespaces")
                .join(&project.urn().encode_id())
                .join("refs")
                .join("remotes")
                .join(peer_id.to_string())
                .join("heads")
                .join("master"),
            format!("{}", commit),
        )
        .unwrap();

        // add a patch
        let cobs = Store::new(whoami, profile.paths(), &storage);
        let patches = cobs.patches();
        let target = MergeTarget::Upstream;
        let base = Oid::from_str("af08e95ada2bb38aadd8e6cef0963ce37a87add3").unwrap();
        let rev0_oid = Oid::from_str("518d5069f94c03427f694bb494ac1cd7d1339380").unwrap();
        let project_urn = &project.urn();
        let _patch_id = patches
            .create(
                project_urn,
                "My first patch",
                "Blah blah blah.",
                target,
                base,
                rev0_oid,
                &[],
            )
            .unwrap();

        // add an issue
        let issues = cobs.issues();
        let issue_id = issues
            .create(&project.urn(), "My first issue", "Blah blah blah.", &[])
            .unwrap();
        issues
            .comment(&project.urn(), &issue_id, "Ho ho ho.")
            .unwrap();

        (profile, signer, project, commit)
    }
}
