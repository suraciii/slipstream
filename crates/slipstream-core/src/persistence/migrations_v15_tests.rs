
    #[tokio::test]
    async fn initializes_current_schema_and_runs_fifo_writes() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(persistence.probe().await.unwrap(), 1);
        persistence.write_probe().await.unwrap();
        assert_eq!(persistence.probe().await.unwrap(), 3);
        let (configuration_send, configuration_receive) = oneshot::channel();
        persistence
            .submit(Command::Configuration(configuration_send))
            .unwrap();
        assert_eq!(
            configuration_receive.await.unwrap().unwrap(),
            ("delete".to_owned(), 1)
        );
        persistence.shutdown().unwrap();
        let connection = Connection::open(path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
    }

    #[tokio::test]
    async fn v14_selection_state_migration_maps_legacy_values_to_canonical_values() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v14.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available)
                   VALUES
                     ('original-one','one.jpg','jpeg',1,1,1),
                     ('original-two','two.jpg','jpeg',1,1,1),
                     ('original-three','three.jpg','jpeg',1,1,1);
                 INSERT INTO photos(
                   id,original_id,available,preview_state,sort_path,selection_state,rating
                 ) VALUES
                   ('photo-one','original-one',1,'inspection-pending','one.jpg','undecided',1),
                   ('photo-two','original-two',1,'inspection-pending','two.jpg','selected',2),
                   ('photo-three','original-three',1,'inspection-pending','three.jpg','rejected',3);",
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.snapshot().await.unwrap();
        persistence.shutdown().unwrap();

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            15
        );
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
        let values: Vec<(String, String)> = connection
            .prepare("SELECT id,selection_state FROM photos ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            values,
            vec![
                ("photo-one".to_owned(), "unflagged".to_owned()),
                ("photo-three".to_owned(), "rejected".to_owned()),
                ("photo-two".to_owned(), "picked".to_owned()),
            ]
        );
        assert_eq!(
            snapshot
                .photos
                .iter()
                .map(|photo| photo.selection_state)
                .collect::<Vec<_>>(),
            vec![
                SelectionState::Unflagged,
                SelectionState::Rejected,
                SelectionState::Picked,
            ]
        );
    }
