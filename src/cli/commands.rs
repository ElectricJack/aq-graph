use super::*;

pub(super) async fn run(client: Client, command: Command) -> Result<Value> {
    match command {
        Command::Task { command } => task(&client, command).await,
        Command::Knowledge { command } => knowledge(&client, command).await,
        Command::Link { command } => link(&client, command).await,
        Command::Search {
            query,
            page,
            kind,
            filters,
            include_disputed,
        } => {
            let mut parameters = page_query(page);
            parameters.push(("q", query));
            if let Some(kind) = kind {
                parameters.push(("kind", kind));
            }
            date_query(&mut parameters, filters);
            if include_disputed {
                parameters.push(("include_disputed", "true".into()));
            }
            client.get("/search", parameters).await
        }
        Command::Summary {
            id,
            change,
            source_revision,
            file,
            clear,
        } => {
            let summary = if clear {
                None
            } else {
                Some(read_text(file.as_deref().ok_or_else(|| {
                    invalid("summary requires --file or --clear")
                })?)?)
            };
            client.mutation(vec![json!({"op":"set_summary","id":id,"expected_version":change.expected_version,"source_revision":source_revision,"summary":summary})],change.reason,change.idempotency_key).await
        }
        Command::History { id, page } => {
            client
                .get(&format!("/nodes/{id}/history"), page_query(page))
                .await
        }
        Command::Revision { id, revision } => {
            client
                .get(&format!("/nodes/{id}/revisions/{revision}"), vec![])
                .await
        }
        Command::Diff { id, from, to } => {
            client
                .get(
                    &format!("/nodes/{id}/diff"),
                    vec![("from", from.to_string()), ("to", to.to_string())],
                )
                .await
        }
        Command::Mutate { file } => client.post("/mutations", read_json(&file)?).await,
        Command::Changeset { command } => changeset(&client, command).await,
        Command::Init { .. } | Command::Serve(_) | Command::Health => unreachable!(),
    }
}

async fn task(client: &Client, command: TaskCommand) -> Result<Value> {
    match command {
        TaskCommand::Create(options) => options.create(client).await,
        TaskCommand::Show(options) => options.show(client).await,
        TaskCommand::List { page, filters } => {
            client
                .get("/nodes", list_query("task", page, filters))
                .await
        }
        TaskCommand::Ready { page } => client.get("/ready", page_query(page)).await,
        TaskCommand::Tree(options) => options.fetch(client).await,
        TaskCommand::SetParent(options) => options.set_parent(client).await,
        TaskCommand::Update(options) => options.update(client).await,
        TaskCommand::Verify { id, options } => client.verify(id, options).await,
        TaskCommand::Retire { id, options } => client.retire(id, options).await,
        TaskCommand::Activate { id, change } | TaskCommand::Reopen { id, change } => {
            client
                .patch(id, change, json!({"task_state":"active"}))
                .await
        }
        TaskCommand::Complete { id, change } => {
            client
                .patch(id, change, json!({"task_state":"completed"}))
                .await
        }
        TaskCommand::Cancel { id, change } => {
            client
                .patch(id, change, json!({"task_state":"canceled"}))
                .await
        }
        TaskCommand::Delete { id, change } => {
            change.submit(client, id, json!({"op":"delete_node"})).await
        }
        TaskCommand::Restore(options) => options.restore(client).await,
    }
}

async fn knowledge(client: &Client, command: KnowledgeCommand) -> Result<Value> {
    match command {
        KnowledgeCommand::Create { mut options } => {
            let operation = create_operation("knowledge", &mut options)?;
            client
                .mutation(vec![operation], options.reason, options.idempotency_key)
                .await
        }
        KnowledgeCommand::Show { id } => client.get(&format!("/nodes/{id}"), vec![]).await,
        KnowledgeCommand::List { page, filters } => {
            client
                .get("/nodes", list_query("knowledge", page, filters))
                .await
        }
        KnowledgeCommand::Update { id, mut options } => {
            let patch = update_patch(&mut options)?;
            client.patch(id, options.change, Value::Object(patch)).await
        }
        KnowledgeCommand::Verify { id, options } => client.verify(id, options).await,
        KnowledgeCommand::Retire { id, options } => client.retire(id, options).await,
        KnowledgeCommand::Restore(options) => options.restore(client).await,
    }
}

async fn link(client: &Client, command: LinkCommand) -> Result<Value> {
    match command {
        LinkCommand::Add {
            source,
            target,
            edge_type,
            source_revision,
            target_revision,
            description,
            reason,
            idempotency_key,
        } => {
            let operation = json!({
                "op": "add_edge",
                "source": source,
                "target": target,
                "edge_type": edge_type,
                "source_revision": source_revision,
                "target_revision": target_revision,
                "description": description
            });
            client
                .mutation(vec![operation], reason, idempotency_key)
                .await
        }
        LinkCommand::Remove { id, change } => {
            let operation = json!({
                "op": "remove_edge",
                "id": id,
                "expected_version": change.expected_version
            });
            client
                .mutation(vec![operation], change.reason, change.idempotency_key)
                .await
        }
    }
}

async fn changeset(client: &Client, command: DraftCommand) -> Result<Value> {
    match command {
        DraftCommand::Create { file } => client.post("/changesets",read_json(&file)?).await,
        DraftCommand::Inspect { id } => client.get(&format!("/changesets/{id}"),vec![]).await,
        DraftCommand::Validate { id } => client.post(&format!("/changesets/{id}/validate"),json!({})).await,
        DraftCommand::Apply { id,idempotency_key } => client.post(&format!("/changesets/{id}/apply"),json!({"idempotency_key":idempotency_key.unwrap_or_else(|| Uuid::new_v4().to_string())})).await,
    }
}

fn list_query(kind: &str, page: Paging, filters: DateFilters) -> Vec<(&'static str, String)> {
    let mut query = page_query(page);
    query.push(("kind", kind.into()));
    date_query(&mut query, filters);
    query
}

fn traversal_query(depth: u32, limit: u32, max_bytes: u32) -> Vec<(&'static str, String)> {
    vec![
        ("depth", depth.to_string()),
        ("limit", limit.to_string()),
        ("max_bytes", max_bytes.to_string()),
    ]
}

impl ShowTask {
    async fn show(self, client: &Client) -> Result<Value> {
        if !self.context {
            return client.get(&format!("/nodes/{}", self.id), vec![]).await;
        }
        let mut query = traversal_query(self.depth, self.limit, self.max_bytes);
        query.push(("direction", self.direction));
        client
            .get(&format!("/nodes/{}/context", self.id), query)
            .await
    }
}

impl RestoreOptions {
    async fn restore(self, client: &Client) -> Result<Value> {
        self.change.submit(client, self.id, json!({"op": "restore_node", "revision": self.revision, "include_lifecycle": self.include_lifecycle})).await
    }
}

impl CreateTask {
    async fn create(mut self, client: &Client) -> Result<Value> {
        let mut operation = create_operation("task", &mut self.options)?;
        operation["task_state"] = json!(self.state);
        operation["acceptance_criteria"] = json!(self.acceptance_criteria);
        client
            .mutation(
                vec![operation],
                self.options.reason,
                self.options.idempotency_key,
            )
            .await
    }
}

impl UpdateTask {
    async fn update(mut self, client: &Client) -> Result<Value> {
        let mut patch = update_patch(&mut self.options)?;
        if let Some(value) = self.acceptance_criteria {
            patch.insert("acceptance_criteria".into(), json!(value));
        }
        client
            .patch(self.id, self.options.change, Value::Object(patch))
            .await
    }
}

impl TaskTree {
    async fn fetch(self, client: &Client) -> Result<Value> {
        client
            .get(
                &format!("/nodes/{}/tree", self.id),
                traversal_query(self.depth, self.limit, self.max_bytes),
            )
            .await
    }
}

impl ParentOptions {
    async fn set_parent(self, client: &Client) -> Result<Value> {
        self.change
            .submit(
                client,
                self.id,
                json!({"op": "set_parent", "parent_id": self.parent}),
            )
            .await
    }
}

impl ChangeOptions {
    async fn submit(self, client: &Client, id: Uuid, mut operation: Value) -> Result<Value> {
        operation["id"] = json!(id);
        operation["expected_version"] = json!(self.expected_version);
        client
            .mutation(vec![operation], self.reason, self.idempotency_key)
            .await
    }
}
