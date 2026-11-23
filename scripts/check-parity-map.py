#!/usr/bin/env python3
"""Extract/check the pinned v1 test inventory without importing or executing v1."""
import argparse
import ast
from collections import Counter
import fnmatch
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MAP = ROOT / 'docs/compatibility/v1-test-inventory.json'
PATTERNS = [
    'tests/test_record*.py', 'tests/test_knowledge*.py',
    'tests/test_hierarchy*.py', 'tests/test_dependency_queries.py',
    'tests/test_task_graph*.py', 'tests/perf/test_hierarchy_statements.py',
]
# These are scope triage decisions based on module contracts and the AST evidence
# retained in every row. They are not claims that similarly named Rust tests pass.
PROFILES = {
    'test_record_backfill.py': ('migration', 'Legacy task/record aliases and backfill reconciliation require an explicit v1 import adapter.'),
    'test_record_doctor.py': ('aq_integration', 'AQ doctor diagnostics/repair are not a standalone graph API surface.'),
    'test_record_exports.py': ('aq_integration', 'AQ vault export ownership, filesystem confinement and divergence workers are outside standalone persistence.'),
    'test_record_outbox.py': ('optional_integration', 'The v1 durable export/index consumer and outbox protocol is not implemented by this synchronous baseline.'),
    'test_record_contracts.py': ('aq_integration', 'AQ CommandHandler/generated command registration and worker grants are adapter-owned contracts.'),
    'test_record_queries.py': ('migration', 'Legacy task aliases, installation UUIDs and SQLAlchemy transaction seams need an import/identity adapter.'),
    'test_record_graph.py': ('graph_core', 'Port scoped graph reads, exact pins, paging and separation of informational links from execution behavior.'),
    'test_record_links.py': ('graph_core', 'Port relationship invariants, exact/floating pins, replay, atomicity and concurrent duplicate behavior with v2 semantics explicit.'),
    'test_record_schema.py': ('knowledge_core', 'Port portable identity/content/history/link/provenance invariants; v1 SQLAlchemy tables and trigger internals are not identical.'),
    'test_dependency_queries.py': ('graph_core', 'Port typed edge queries, scope and duplicate/replay semantics; unsupported dependency types must remain explicit.'),
    'test_hierarchy_queries.py': ('graph_core', 'Port structural graph invariants and bounded reads; pointer columns, dotted IDs and AQ container lifecycle need separate adaptation.'),
    'test_hierarchy_archive_delete.py': ('aq_orchestration', 'AQ subtree archive/cascade, completion settlement, delivery holds and integration-history dispositions differ from v2 tombstones.'),
    'test_hierarchy_commands.py': ('aq_orchestration', 'AQ task-close/abandon/session/branch-cleanup command policy is not intrinsic graph readiness.'),
    'test_hierarchy_settlement.py': ('aq_orchestration', 'Automatic container completion, sessions, integration episodes and event fan-out are AQ policy.'),
    'test_hierarchy_ids.py': ('migration', 'Dotted IDs, child ordinals and permanent AQ integration identity reservations are not v2 opaque UUID allocation.'),
    'test_hierarchy_migration.py': ('migration', 'Legacy parent-column/edge canonicalization and ordinal backfill require a dedicated import migration.'),
    'test_hierarchy_doctor.py': ('aq_integration', 'AQ hierarchy doctor/repair and legacy parent-pointer diagnostics are not a standalone graph endpoint.'),
    'test_hierarchy_graph_creator.py': ('aq_integration', 'AQ graph-document planning, dotted ID reservation and routing integration are adapter-owned.'),
    'test_task_graph.py': ('aq_integration', 'AQ YAML/spec parser, variables, phases, routing and formula creator are not the v2 MutationRequest contract.'),
    'test_task_graph_activity.py': ('aq_orchestration', 'Dashboard activity, routing cache, delivery holds and running-session rollups remain AQ-owned.'),
    'test_task_graph_live_events.py': ('aq_integration', 'AQ in-process EventBus/dashboard notifications and creation pipelines are separate from committed graph HTTP reads.'),
    'test_hierarchy_statements.py': ('performance', 'SQLAlchemy statement-count budgets need a distinct v2 query-plan/performance translation; latency measurements are not equivalent coverage.'),
    'test_knowledge_service.py': ('knowledge_core', 'Port canonical content, bounded snapshots, CAS/replay/rollback, history and scoped lexical reads.'),
    'test_knowledge_authority.py': ('knowledge_core', 'Port verification/protection/evidence and current-access checks; AQ review bindings and global-share policy need explicit adapters.'),
    'test_knowledge_authorization.py': ('knowledge_core', 'Port fail-closed ownership/scope/live-capability/replay checks; AQ held-task/session claims are not v2 principals.'),
    'test_knowledge_task_creation.py': ('aq_integration', 'AQ Save finding/task filing combines routing, gates, held claims and filing allowances; an atomic create-plus-link graph batch is only its portable subset.'),
    'test_knowledge_commands.py': ('knowledge_core', 'Port portable create/show/update/history/search/link behavior; AQ command registration/activation gates remain adapter-owned.'),
    'test_knowledge_proposals.py': ('knowledge_core', 'Port exact-base proposals, atomic application, stale conflict and decision authority using v2 draft change sets.'),
    'test_knowledge_redaction.py': ('migration', 'One-way erasure, purge recovery and backup/index denial require the separate administrative erasure policy explicitly deferred by the v2 spec.'),
    'test_knowledge_context.py': ('aq_integration', 'AQ prompt bundles, prime/resume, model budgets, delivery receipts and supervisor instances are separate from graph context neighborhoods.'),
    'test_knowledge_budget.py': ('aq_orchestration', 'LLM input/output/tool token budgeting and operator model windows are outside the graph service.'),
    'test_knowledge_capture.py': ('optional_integration', 'Durable execution-result capture, wake reconciliation and generation input retention require AQ execution integration.'),
    'test_knowledge_citations.py': ('aq_integration', 'Observed harness delivery/citations are execution evidence; graph revision pins alone do not establish injection.'),
    'test_knowledge_compatibility.py': ('migration', 'Legacy memory cutover, managed vault paths and write fences require the explicitly scoped migration adapter.'),
    'test_knowledge_deprecation.py': ('migration', 'Legacy removal gates, inventories, restore seals and telemetry are migration/operator obligations.'),
    'test_knowledge_evaluation.py': ('aq_integration', 'V1 offline harness oracles/context fixture adapters need their own translation; a fixture hash is not graph behavioral coverage.'),
    'test_knowledge_extraction.py': ('optional_provider', 'Paid generation, provider circuits, reservations and proposal synthesis are optional systems outside model-free core reads.'),
    'test_knowledge_extraction_store.py': ('optional_provider', 'Generation leases, paid-call fences and token/money accounting are optional provider persistence.'),
    'test_knowledge_harness_delivery.py': ('aq_integration', 'Harness hooks, launch/resume delivery, workspace guidance and injected-citation receipts are AQ integration.'),
    'test_knowledge_import.py': ('migration', 'Legacy retained-source import, sealed manifests, receipt reconciliation and restore drills require a dedicated importer.'),
    'test_knowledge_import_inventory.py': ('migration', 'Legacy read-only import inventory and cutover schema are not ordinary graph create/update.'),
    'test_knowledge_inventory.py': ('migration', 'Filesystem/vector export inventory, sealed evidence and reconciliation belong to migration tooling.'),
    'test_knowledge_migrations.py': ('migration', 'V1 Alembic helper installation, downgrade guards and SQLAlchemy metadata are not v2 SQLx migrations.'),
    'test_knowledge_index_receipts.py': ('optional_provider', 'Optional semantic index manifests, provider registration and erasure acknowledgments have no baseline v2 backend.'),
    'test_knowledge_memory_adapter.py': ('optional_provider', 'Legacy aq-memory plugin adaptation/semantic rankings are optional and excluded from authoritative model-free graph reads.'),
    'test_knowledge_provider_absence.py': ('optional_provider', 'Provider degradation and master-switch tests require optional registration/search interfaces absent from v2.'),
    'test_knowledge_retrieval.py': ('optional_provider', 'Provider-ranked references, hydration races and semantic-search access checks require optional ranking integration.'),
    'test_knowledge_retrieval_benchmark.py': ('optional_provider', 'Semantic-vs-lexical quality oracles and G5 gates cannot be inferred from PostgreSQL lexical or load benchmarks.'),
}
CORE_OVERRIDES = {
    'test_record_contracts.py': {'test_category_is_a_seven_value_closed_enum','test_idempotency_key_bounds','test_search_limit_bound','test_link_create_requires_at_least_one_operation','test_knowledge_create_requires_the_core_fields','test_args_models_are_closed'},
    'test_record_queries.py': {'test_exact_history_cas_and_no_implicit_commit','test_task_archive_and_delete_preserve_mapping_and_exact_projection','test_live_task_delete_is_not_pinned_by_record'},
    'test_task_graph.py': {'TestSelfEdges.test_self_edge_is_rejected_for_every_dep_type','TestSelfEdges.test_a_genuine_two_node_edge_is_untouched','test_validator_reports_blocking_cycle_but_allows_related_cycle','TestCreateGraph.test_failure_on_node_three_leaves_zero_rows','TestCreateGraph.test_labels_are_written','TestCreateGraph.test_duplicate_labels_do_not_break_the_insert','TestCreateGraph.test_dry_run_writes_nothing_but_reports_ids'},
    'test_hierarchy_archive_delete.py': {'TestArchive.test_completion_story_survives_archive'},
    'test_hierarchy_commands.py': {'TestHierarchyCommands.test_children_flat_and_recursive','TestHierarchyCommands.test_reparent_and_root','TestHierarchyCommands.test_reparent_error_codes','TestHierarchyCommands.test_reparent_to_current_parent_is_idempotent'},
    'test_knowledge_context.py': {'test_exact_old_pin_after_edit_is_never_substituted','test_cross_project_pin_does_not_leak_private_metadata','test_lower_budget_drops_whole_items_with_omissions'},
    'test_knowledge_citations.py': {'test_explicit_reads_and_attachments_pin_exact_revision_independent_of_injection'},
    'test_knowledge_task_creation.py': {'test_replay_is_one_task_and_pinned_informational_link','test_link_failure_rolls_back_task_gate_and_receipt','test_save_selected_finding_atomically_links_task_and_replays'},
}
AQ_HIERARCHY = {
    'test_runnable_candidate_reads_use_one_array_bind','test_create_task_under_locks_project_before_child_ordinal',
    'test_live_descendant_sessions_reports_holder_of_any_subtree_task','test_live_descendant_sessions_can_exclude_the_root','test_abandon_subtree_releases_each_descendant_resource_once',
    'TestSchemaFields.test_new_task_columns_have_defaults','TestSchemaFields.test_created_by_round_trips','TestSchemaFields.test_agent_state_has_retired',
    'TestCreateTaskUnder.test_mints_dotted_id_and_links','TestCreateTaskUnder.test_naming_cap_uses_discovered_from',
    'TestSetParentReblocksWaitsFor.test_moving_the_last_open_child_away_unblocks_the_old_waiter',
    'TestCreateTaskCommandHierarchyErrors.test_completed_parent_returns_hierarchy_error',
    'TestMarkContainerIdempotent.test_called_twice_leaves_one_row','TestReads.test_progress_extras',
    'TestSetParentBulk.test_enabled_project_rejects_bulk_parent_writer',
    'TestAbandonReleasesResources.test_workspace_lock_and_agent_pointer_are_cleared',
}

def git(source, *arguments):
    return subprocess.check_output(['git','-C',str(source),*arguments],text=True)

def selected(path):
    # fnmatch '*' spans slashes; enforce the requested direct tests/ family paths.
    return any(fnmatch.fnmatchcase(path,pattern) and (path.count('/')==pattern.count('/')) for pattern in PATTERNS)


def qualified_tests(nodes, scope="", decorators=()):
    for node in nodes:
        if isinstance(node, ast.ClassDef):
            yield from qualified_tests(
                node.body,
                scope + node.name + ".",
                decorators + tuple(node.decorator_list),
            )
        elif isinstance(
            node, (ast.FunctionDef, ast.AsyncFunctionDef)
        ) and node.name.startswith("test_"):
            yield scope + node.name, node, decorators + tuple(node.decorator_list)


def size(node, bindings, depth=0):
    if depth > 10:
        return None
    match node:
        case (
            ast.List(elts=items)
            | ast.Tuple(elts=items)
            | ast.Set(elts=items)
            | ast.Dict(keys=items)
        ):
            return len(items)
        case ast.Name(id=name) if name in bindings:
            return size(bindings[name], bindings, depth + 1)
        case ast.Call(func=ast.Name(id="range"), args=args):
            return range_size(args)
        case ast.Call(func=ast.Name(id=("list" | "tuple" | "sorted")), args=[argument]):
            return size(argument, bindings, depth + 1)
        case ast.BinOp(op=ast.Add(), left=left, right=right):
            sizes = [size(part, bindings, depth + 1) for part in (left, right)]
            return None if None in sizes else sum(sizes)
        case _:
            return None


def range_size(arguments):
    if not all(
        isinstance(arg, ast.Constant) and type(arg.value) is int for arg in arguments
    ):
        return None
    try:
        return len(range(*(arg.value for arg in arguments)))
    except (TypeError, ValueError, OverflowError):
        return None


def param_info(decorators, bindings):
    parameters = [
        parameter(decorator, bindings)
        for decorator in decorators
        if isinstance(decorator, ast.Call)
        and ast.unparse(decorator.func).endswith(".parametrize")
    ]
    counts = [entry["static_values_count"] for entry in parameters]
    combinations = None if None in counts else math.prod(counts)
    return {
        "decorators": parameters,
        "static_combinations": combinations,
        "fixture_expansion": "not_evaluated",
        "note": "AST estimate only; fixtures, dynamic comprehensions, skips and plugin collection are not executed.",
    }


def parameter(decorator, bindings):
    if len(decorator.args) > 1:
        values = decorator.args[1]
    else:
        values = next(
            (kw.value for kw in decorator.keywords if kw.arg == "argvalues"), None
        )
    return {
        "decorator": ast.unparse(decorator),
        "static_values_count": size(values, bindings),
    }


SPECIFIC_PROFILES = {
    "test_hierarchy_queries.py": (
        AQ_HIERARCHY,
        "aq_orchestration",
        "This case asserts AQ container/session/routing/ordinal or automatic lifecycle policy; v2 parent-child edges do not independently block execution or settle tasks.",
    ),
    "test_dependency_queries.py": (
        {
            "test_stuck_defined_tasks_are_listed_once_with_their_route",
            "test_get_stuck_defined_tasks_reads_tasks_that_carry_a_route",
            "test_guarded_dependency_waits_for_hierarchy_then_rechecks_target",
        },
        "aq_orchestration",
        "AQ routing, held hierarchy admission and defined-task recovery are separate from graph-intrinsic readiness.",
    ),
    "test_knowledge_service.py": (
        {
            "test_outbox_failure_rolls_back_revision_and_receipt",
            "test_export_intent_is_private_metadata_only",
        },
        "optional_integration",
        "The assertion includes AQ export/outbox side effects absent from the synchronous v2 baseline; graph atomicity alone is only partial coverage.",
    ),
}


def triage(path, name):
    filename = Path(path).name
    category, reason = PROFILES[filename]
    if name in CORE_OVERRIDES.get(filename, set()):
        category = (
            "knowledge_core"
            if filename.startswith("test_knowledge")
            or filename == "test_record_contracts.py"
            else "graph_core"
        )
        reason = "Portable assertion candidate isolated from the surrounding AQ surface; inspect full source before adapting, and record any omitted v1-only obligations."
    special = SPECIFIC_PROFILES.get(filename)
    if special:
        names, override_category, override_reason = special
        if name in names:
            category, reason = override_category, override_reason
    return disposition(filename, category, reason)


def disposition(filename, category, reason):
    if category == "migration" and filename != "test_knowledge_redaction.py":
        return (
            category,
            "P2",
            "excluded",
            "Outside aq-graph product scope: v1 import, cutover, legacy identity and old-product schema compatibility are reference evidence only. AQ v2 is a fresh start; any optional owner-specific transfer is an external script, not a supported aq-graph feature.",
        )
    if category in ("graph_core", "knowledge_core"):
        return (
            category,
            "P0"
            if filename
            in {
                "test_record_schema.py",
                "test_record_links.py",
                "test_knowledge_service.py",
                "test_knowledge_authority.py",
                "test_knowledge_authorization.py",
                "test_knowledge_proposals.py",
            }
            else "P1",
            "needs_translation",
            reason
            + " No v2 behavioral equivalence is claimed until assertions and runtime results are verified.",
        )
    return (
        category,
        "P2" if category in ("migration", "performance") else "P3",
        "deferred",
        reason
        + " Explicit scope/adapter work is required; this is not counted as v2 parity.",
    )


def extract(source, revision):
    paths = [
        p
        for p in git(
            source, "ls-tree", "-r", "--name-only", revision, "tests"
        ).splitlines()
        if selected(p)
    ]
    records = []
    files = []
    for path in paths:
        text = git(source, "show", f"{revision}:{path}")
        source_file, definitions = inspect_test_module(path, text)
        files.append(source_file)
        records.extend(definitions)
    return {
        "schema_version": 1,
        "source": {
            "repository": "agent-queue2",
            "revision": revision,
            "selection_patterns": PATTERNS,
            "method": "git show pinned blobs + Python AST; no v1 imports, pytest collection, tests, daemon, database, or mutations",
        },
        "files": files,
        "tests": records,
    }


def inspect_test_module(path, text):
    tree = ast.parse(text, filename=path)
    module_doc = (ast.get_docstring(tree) or "").split("\n\n")[0]
    bindings = module_bindings(tree)
    tests = list(qualified_tests(tree.body))
    source_file = {
        "path": path,
        "sha256": hashlib.sha256(text.encode()).hexdigest(),
        "test_definitions": len(tests),
        "module_behavior": module_doc,
    }
    return source_file, [test_record(path, text, test, bindings) for test in tests]


def module_bindings(tree):
    return {
        target.id: node.value
        for node in tree.body
        if isinstance(node, ast.Assign)
        for target in node.targets
        if isinstance(target, ast.Name)
    }


def test_record(path, text, test, bindings):
    name, node, decorators = test
    segment = ast.get_source_segment(text, node)
    assertions = [
        ast.unparse(n.test) for n in ast.walk(node) if isinstance(n, ast.Assert)
    ]
    calls = list(
        dict.fromkeys(
            ast.unparse(n.func) for n in ast.walk(node) if isinstance(n, ast.Call)
        )
    )
    category, priority, status, reason = triage(path, name)
    return {
        "id": path + "::" + name,
        "source": {
            "path": path,
            "function": name,
            "line": node.lineno,
            "end_line": node.end_lineno,
            "sha256": hashlib.sha256(segment.encode()).hexdigest(),
            "docstring": ast.get_docstring(node),
            "assertion_count": len(assertions),
            "assertion_samples": [a[:300] for a in assertions[:4]],
            "called_operations": [c[:160] for c in calls[:24]],
            "called_operations_truncated": len(calls) > 24,
            "parameterization": param_info(decorators, bindings),
        },
        "category": category,
        "priority": priority,
        "status": status,
        "reason": reason,
        "v2_tests": [],
        "candidate_v2_tests": [],
        "verification": {"result": "not_run", "command": None},
        "coverage_scope": "none",
    }


def summary(data):
    rows = data["tests"]
    parameters = [r["source"]["parameterization"] for r in rows]
    counts = [
        p["static_combinations"]
        for p in parameters
        if p["static_combinations"] is not None
    ]

    def by(field):
        return dict(sorted(Counter(row[field] for row in rows).items()))

    return {
        "source_files": len(data["files"]),
        "test_definitions": len(rows),
        "by_category": by("category"),
        "by_status": by("status"),
        "by_priority": by("priority"),
        "parameterized_definitions": sum(bool(p["decorators"]) for p in parameters),
        "static_combinations_known": len(counts),
        "sum_known_static_combinations": sum(counts),
        "dynamic_parameter_definitions": len(parameters) - len(counts),
        "count_caveat": "Definitions are not pytest collected cases. Known static combinations exclude unresolved decorator values and all fixture/plugin expansion.",
    }


def validate_verification(row):
    ident = row["id"]
    errors = []
    verification = row.get("verification", {})
    verified = verification.get("result") == "passed" and verification.get("command")
    if not row.get("v2_tests") or not verified:
        errors.append(
            "Verified status needs exact targets and passing command: " + ident
        )
    if row.get("coverage_scope") != "portable_obligations":
        errors.append("Partial assertions must remain needs_translation: " + ident)
    return errors


def check_rust_target(ident, target):
    errors = []
    path = ROOT / target["path"]
    function = target["function"].split("::")[-1]
    if not path.is_file() or not re.search(
        r"\bfn\s+" + re.escape(function) + r"\s*\(", path.read_text()
    ):
        errors.append("Missing Rust target: " + ident + " -> " + str(target))
    if not target.get("assertions"):
        errors.append("Target needs assertion correspondence: " + ident)
    return errors


def validate_row(row):
    ident = row["id"]
    errors = []
    if row["status"] not in (
        "needs_translation",
        "existing_coverage",
        "translated",
        "deferred",
        "excluded",
    ):
        errors.append("Invalid status: " + ident)
    if not row.get("reason"):
        errors.append("Missing reason: " + ident)
    if row["status"] in ("existing_coverage", "translated"):
        errors.extend(validate_verification(row))
    for target in row.get("v2_tests", []) + row.get("candidate_v2_tests", []):
        errors.extend(check_rust_target(ident, target))
    return errors


def validate(data, source=None):
    errors = []
    seen = set()
    if data.get("schema_version") != 1:
        errors.append("Unsupported inventory schema_version")
    for row in data["tests"]:
        ident = row["id"]
        if ident in seen:
            errors.append("Duplicate id: " + ident)
        seen.add(ident)
        errors.extend(validate_row(row))
    if data.get("summary") != summary(data):
        errors.append("Summary is stale; regenerate with --refresh")
    if source is not None:
        errors.extend(check_source(data, source))
    return errors


def test_sources(inventory):
    return {row["id"]: row["source"] for row in inventory["tests"]}


def file_hashes(inventory):
    return {entry["path"]: entry["sha256"] for entry in inventory["files"]}


def check_source(data, source):
    errors = []
    fresh = extract(source, data["source"]["revision"])
    if fresh["files"] != data["files"]:
        errors.append("Pinned file hashes or definition counts differ")
    if test_sources(data) != test_sources(fresh):
        errors.append(
            "Pinned AST evidence differs or test definitions are missing/extra"
        )
    return errors


def copy_reviewed_fields(row, prior):
    keys = (
        "category",
        "priority",
        "status",
        "reason",
        "v2_tests",
        "candidate_v2_tests",
        "verification",
        "coverage_scope",
        "notes",
        "schema5_verification",
    )
    row.update({key: prior[key] for key in keys if key in prior})


def retain_reviewed_dispositions(previous, data):
    old = {r["id"]: r for r in previous["tests"]}
    old_files = file_hashes(previous)
    new_files = file_hashes(data)
    for row in data["tests"]:
        prior = old.get(row["id"])
        if prior is None or prior["source"]["sha256"] != row["source"]["sha256"]:
            continue
        path = row["source"]["path"]
        if old_files.get(path) == new_files.get(path):
            copy_reviewed_fields(row, prior)


def refresh(args):
    previous = (
        json.loads(args.inventory.read_text()) if args.inventory.exists() else None
    )
    requested_revision = args.revision or (
        previous["source"]["revision"] if previous else "HEAD"
    )
    revision = git(
        args.source_root, "rev-parse", "--verify", requested_revision + "^{commit}"
    ).strip()
    data = extract(args.source_root, revision)
    if previous:
        retain_reviewed_dispositions(previous, data)
    data["summary"] = summary(data)
    args.inventory.parent.mkdir(parents=True, exist_ok=True)
    args.inventory.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    return data


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source-root",
        type=Path,
        help="Read-only v1 git checkout; required for refresh and optional for full source verification",
    )
    parser.add_argument(
        "--revision",
        help="Revision to pin on initial generation; existing inventories keep their recorded revision",
    )
    parser.add_argument("--inventory", type=Path, default=DEFAULT_MAP)
    parser.add_argument(
        "--refresh",
        action="store_true",
        help="Generate/update source AST evidence while preserving unchanged reviewed dispositions",
    )
    args = parser.parse_args()
    if args.refresh and not args.source_root:
        parser.error("--refresh requires --source-root")
    return args


def main():
    args = parse_args()
    data = refresh(args) if args.refresh else json.loads(args.inventory.read_text())
    errors = validate(data, args.source_root)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps(data["summary"], indent=2))


if __name__ == "__main__":
    main()
