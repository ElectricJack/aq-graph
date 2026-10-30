-- ORDER BY id / LIMIT search plans must not walk unrelated task records when
-- knowledge IDs sort after a large task population. Full-text candidate lookup
-- retains its GIN index; this index also supports scope-first ordered scans.
CREATE INDEX nodes_knowledge_page ON nodes(workspace_id,id)
    WHERE NOT deleted AND kind='knowledge';
