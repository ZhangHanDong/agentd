-- M3 Plan C: the durable link between a task-graph node and the M2 durable
-- scheduler queue row that executes it. Before this, graph dispatch could only
-- send a direct message and wait for a `task_graph_result` reply; a node with
-- an `execution` spec now gets a `task_runs` row and an `execution_task_queue`
-- entry, and the daemon settles the node from that row's terminal status.
-- The (graph_id, node_id) primary key is what makes re-advancing a node
-- idempotent after a crash between task creation and the graph write.
CREATE TABLE task_graph_node_executions (
    graph_id          TEXT NOT NULL CHECK (length(trim(graph_id)) > 0),
    node_id           TEXT NOT NULL CHECK (length(trim(node_id)) > 0),
    execution_task_id TEXT NOT NULL UNIQUE REFERENCES task_runs(id),
    run_id            TEXT NOT NULL,
    settled           INTEGER NOT NULL DEFAULT 0 CHECK (settled IN (0, 1)),
    created_at        INTEGER NOT NULL,
    settled_at        INTEGER,
    PRIMARY KEY (graph_id, node_id)
);

CREATE INDEX idx_task_graph_node_executions_open
    ON task_graph_node_executions(settled, execution_task_id);

UPDATE schema_meta SET value = '27' WHERE key = 'version';
