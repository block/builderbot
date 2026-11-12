//! Tag catalog, assignment and legacy-import rules (migration 0045).

use serde_json::json;
use tokio::sync::broadcast;

use super::*;

// =============================================================================
// Helpers
// =============================================================================

fn store() -> Store {
    Store::in_memory().unwrap()
}

fn store_with_feed() -> (Store, broadcast::Receiver<StoreChange>) {
    let (tx, rx) = broadcast::channel(256);
    (Store::in_memory().unwrap().with_change_sender(tx), rx)
}

fn drain(rx: &mut broadcast::Receiver<StoreChange>) -> Vec<StoreChange> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn named(name: &str) -> NewTag {
    NewTag {
        name: name.to_string(),
        color: None,
        icon: None,
    }
}

fn project(store: &Store, name: &str) -> Project {
    let project = Project::named(name);
    store.create_project(&project).unwrap();
    project
}

fn tag(store: &Store, name: &str, group_id: Option<&str>) -> Tag {
    store.create_tag(&named(name), group_id, None).unwrap()
}

fn group(store: &Store, name: &str, tags: &[&str]) -> TagGroup {
    let tags: Vec<NewTag> = tags.iter().map(|name| named(name)).collect();
    store.create_tag_group(name, &tags).unwrap()
}

/// A scope's tag names in position order, asserting the positions are dense.
fn scope_names(store: &Store, group_id: Option<&str>) -> Vec<String> {
    let mut tags: Vec<Tag> = store
        .get_tag_state()
        .unwrap()
        .tags
        .into_iter()
        .filter(|tag| tag.group_id.as_deref() == group_id)
        .collect();
    tags.sort_by_key(|tag| tag.position);
    let positions: Vec<i64> = tags.iter().map(|tag| tag.position).collect();
    assert_eq!(positions, (0..tags.len() as i64).collect::<Vec<_>>());
    tags.into_iter().map(|tag| tag.name).collect()
}

/// Group names in position order, asserting the positions are dense.
fn group_names(store: &Store) -> Vec<String> {
    let groups = store.get_tag_state().unwrap().groups;
    let positions: Vec<i64> = groups.iter().map(|group| group.position).collect();
    assert_eq!(positions, (0..groups.len() as i64).collect::<Vec<_>>());
    groups.into_iter().map(|group| group.name).collect()
}

fn tag_ids_of(store: &Store, project_id: &str) -> Vec<String> {
    store
        .get_tag_state()
        .unwrap()
        .assignments
        .into_iter()
        .filter(|a| a.project_id == project_id)
        .map(|a| a.tag_id)
        .collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn err<T: std::fmt::Debug>(result: Result<T, StoreError>) -> String {
    result.expect_err("expected an error").0
}

// =============================================================================
// Names, icons and colours
// =============================================================================

#[test]
fn a_name_is_unique_per_scope_ignoring_case() {
    let store = store();
    let priority = group(&store, "Priority", &["High"]);
    let effort = group(&store, "Effort", &[]);

    // "Priority › High" and "Effort › High" coexist.
    tag(&store, "High", Some(&effort.id));
    assert_eq!(
        err(store.create_tag(&named("high"), Some(&priority.id), None)),
        "A tag named “high” already exists in Priority"
    );

    tag(&store, "infra", None);
    assert_eq!(
        err(store.create_tag(&named("Infra"), None, None)),
        "A tag named “Infra” already exists among ungrouped tags"
    );

    assert_eq!(
        err(store.create_tag_group("priority", &[])),
        "A group named “priority” already exists"
    );
    assert_eq!(
        err(store.rename_tag_group(&effort.id, " PRIORITY ")),
        "A group named “PRIORITY” already exists"
    );

    let low = tag(&store, "Low", Some(&priority.id));
    assert_eq!(
        err(store.update_tag(&low.id, "HIGH", TagColor::Red, None)),
        "A tag named “HIGH” already exists in Priority"
    );
}

#[test]
fn names_are_trimmed_and_one_to_sixty_four_characters() {
    let store = store();
    assert_eq!(tag(&store, "  Bug  ", None).name, "Bug");
    assert_eq!(
        err(store.create_tag(&named("   "), None, None)),
        "Tag name can't be empty"
    );
    assert_eq!(
        err(store.create_tag_group("", &[])),
        "Group name can't be empty"
    );
    let long = "é".repeat(64);
    assert_eq!(tag(&store, &long, None).name, long);
    assert_eq!(
        err(store.create_tag(&named(&"x".repeat(65)), None, None)),
        "Tag name must be 64 characters or fewer"
    );
}

#[test]
fn an_icon_is_none_or_a_kebab_case_name() {
    let store = store();
    for icon in ["chevron-up", "circle-dot", "x", "arrow-up-0"] {
        let new = NewTag {
            name: icon.to_string(),
            color: None,
            icon: Some(icon.to_string()),
        };
        assert_eq!(
            store.create_tag(&new, None, None).unwrap().icon.as_deref(),
            Some(icon)
        );
    }
    assert_eq!(tag(&store, "dot", None).icon, None);

    for icon in [
        "",
        "ChevronUp",
        "chevron--up",
        "-x",
        "x-",
        "chevron_up",
        "chevron up",
    ] {
        let new = NewTag {
            name: format!("bad {icon}"),
            color: None,
            icon: Some(icon.to_string()),
        };
        assert_eq!(
            err(store.create_tag(&new, None, None)),
            format!("Invalid tag icon “{icon}”")
        );
    }
}

#[test]
fn a_tag_created_without_a_colour_gets_the_least_used_one() {
    let store = store();
    let colors: Vec<TagColor> = (0..7)
        .map(|i| tag(&store, &format!("t{i}"), None).color)
        .collect();
    // Every colour is unused at first, so ties walk the tie-break order.
    assert_eq!(colors, TagColor::AUTO_ORDER.to_vec());

    // Counted across all tags, grouped or not.
    let g = group(&store, "G", &[]);
    let blue = NewTag {
        name: "explicit".to_string(),
        color: Some(TagColor::Blue),
        icon: None,
    };
    store.create_tag(&blue, Some(&g.id), None).unwrap();
    assert_eq!(tag(&store, "next", None).color, TagColor::Green);
    assert_eq!(tag(&store, "after", Some(&g.id)).color, TagColor::Amber);

    // A group's initial tags see each other's picks.
    let colors: Vec<TagColor> = {
        let created = group(&store, "Fresh", &["a", "b", "c"]);
        store
            .get_tag_state()
            .unwrap()
            .tags
            .into_iter()
            .filter(|t| t.group_id.as_deref() == Some(created.id.as_str()))
            .map(|t| t.color)
            .collect()
    };
    assert_eq!(
        colors,
        vec![TagColor::Purple, TagColor::Cyan, TagColor::Red]
    );
}

// =============================================================================
// Positions
// =============================================================================

#[test]
fn positions_stay_dense_through_create_move_and_delete() {
    let store = store();
    let ids: Vec<String> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| tag(&store, name, None).id)
        .collect();
    assert_eq!(scope_names(&store, None), ["a", "b", "c", "d"]);

    store.delete_tag(&ids[1]).unwrap();
    assert_eq!(scope_names(&store, None), ["a", "c", "d"]);

    store.move_tag(&ids[3], None, 0).unwrap();
    assert_eq!(scope_names(&store, None), ["d", "a", "c"]);

    // The index is clamped.
    store.move_tag(&ids[3], None, 99).unwrap();
    assert_eq!(scope_names(&store, None), ["a", "c", "d"]);

    // Leaving a scope renumbers it; entering one inserts at the index.
    let g = group(&store, "G", &["x", "y"]);
    store.move_tag(&ids[0], Some(&g.id), 1).unwrap();
    assert_eq!(scope_names(&store, None), ["c", "d"]);
    assert_eq!(scope_names(&store, Some(&g.id)), ["x", "a", "y"]);

    let h = group(&store, "H", &["one", "two"]);
    assert_eq!(group_names(&store), ["G", "H"]);
    store
        .delete_tag_group(&g.id, DeleteTagGroupMode::DeleteTags)
        .unwrap();
    assert_eq!(group_names(&store), ["H"]);
    assert_eq!(scope_names(&store, Some(&h.id)), ["one", "two"]);
}

#[test]
fn a_group_is_created_last_with_its_tags_in_order() {
    let store = store();
    group(&store, "First", &[]);
    let second = group(&store, "Second", &["P0", "P1", "P2"]);
    assert_eq!(second.position, 1);
    // First already drives the icon.
    assert!(!second.is_icon_group);
    assert_eq!(scope_names(&store, Some(&second.id)), ["P0", "P1", "P2"]);

    // A bad initial tag rolls the whole group back.
    assert_eq!(
        err(store.create_tag_group("Third", &[named("same"), named("Same")])),
        "A tag named “Same” already exists in Third"
    );
    assert_eq!(group_names(&store), ["First", "Second"]);
}

#[test]
fn reordering_groups_needs_an_exact_permutation() {
    let store = store();
    let a = group(&store, "A", &[]).id;
    let b = group(&store, "B", &[]).id;
    let c = group(&store, "C", &[]).id;

    let expected = "Tag group order must list every tag group exactly once";
    for bad in [
        vec![a.clone(), b.clone()],
        vec![a.clone(), b.clone(), b.clone()],
        vec![a.clone(), b.clone(), c.clone(), c.clone()],
        vec![a.clone(), b.clone(), "unknown".to_string()],
    ] {
        assert_eq!(err(store.reorder_tag_groups(&bad)), expected);
    }
    assert_eq!(group_names(&store), ["A", "B", "C"]);

    store
        .reorder_tag_groups(&[c.clone(), a.clone(), b.clone()])
        .unwrap();
    assert_eq!(group_names(&store), ["C", "A", "B"]);
}

// =============================================================================
// Single choice
// =============================================================================

#[test]
fn a_project_holds_one_tag_per_group_through_both_setters() {
    let store = store();
    let p = project(&store, "p");
    let status = group(&store, "Status", &["Doing", "Done"]);
    let state = store.get_tag_state().unwrap();
    let doing = state
        .tags
        .iter()
        .find(|t| t.name == "Doing")
        .unwrap()
        .id
        .clone();
    let done = state
        .tags
        .iter()
        .find(|t| t.name == "Done")
        .unwrap()
        .id
        .clone();
    let x = tag(&store, "x", None).id;
    let y = tag(&store, "y", None).id;

    store
        .set_project_group_tag(&p.id, &status.id, Some(&doing))
        .unwrap();
    store.set_project_tag_assigned(&p.id, &x, true).unwrap();
    store.set_project_tag_assigned(&p.id, &y, true).unwrap();
    assert_eq!(
        sorted(tag_ids_of(&store, &p.id)),
        sorted(vec![doing.clone(), x.clone(), y.clone()])
    );

    // Assigning a grouped tag through the per-tag setter replaces its sibling.
    store.set_project_tag_assigned(&p.id, &done, true).unwrap();
    assert_eq!(
        sorted(tag_ids_of(&store, &p.id)),
        sorted(vec![done.clone(), x.clone(), y.clone()])
    );

    store
        .set_project_group_tag(&p.id, &status.id, Some(&doing))
        .unwrap();
    assert_eq!(
        sorted(tag_ids_of(&store, &p.id)),
        sorted(vec![doing.clone(), x.clone(), y.clone()])
    );

    store
        .set_project_group_tag(&p.id, &status.id, None)
        .unwrap();
    store.set_project_tag_assigned(&p.id, &x, false).unwrap();
    assert_eq!(tag_ids_of(&store, &p.id), vec![y.clone()]);

    // The tag must belong to the group, and the project must exist.
    let other = group(&store, "Other", &["o"]);
    let o = scope_tag_id(&store, &other.id, "o");
    assert_eq!(
        err(store.set_project_group_tag(&p.id, &status.id, Some(&o))),
        format!("Tag {o} is not in tag group {}", status.id)
    );
    assert_eq!(
        err(store.set_project_group_tag(&p.id, &status.id, Some(&x))),
        format!("Tag {x} is not in tag group {}", status.id)
    );
    assert_eq!(
        err(store.set_project_tag_assigned("nope", &x, true)),
        "Project not found: nope"
    );
}

fn scope_tag_id(store: &Store, group_id: &str, name: &str) -> String {
    store
        .get_tag_state()
        .unwrap()
        .tags
        .into_iter()
        .find(|t| t.group_id.as_deref() == Some(group_id) && t.name == name)
        .unwrap()
        .id
}

#[test]
fn creating_a_tag_for_a_project_assigns_it_in_the_same_write() {
    let (store, mut rx) = store_with_feed();
    let p = project(&store, "p");
    let status = group(&store, "Status", &["Doing"]);
    let doing = scope_tag_id(&store, &status.id, "Doing");
    store
        .set_project_group_tag(&p.id, &status.id, Some(&doing))
        .unwrap();
    drain(&mut rx);

    let done = store
        .create_tag(&named("Done"), Some(&status.id), Some(&p.id))
        .unwrap();
    let q4 = store.create_tag(&named("q4"), None, Some(&p.id)).unwrap();
    assert_eq!(
        sorted(tag_ids_of(&store, &p.id)),
        sorted(vec![done.id, q4.id])
    );
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags, StoreChange::Tags]);

    // An unknown project fails the create too.
    assert_eq!(
        err(store.create_tag(&named("orphan"), None, Some("nope"))),
        "Project not found: nope"
    );
    assert!(!scope_names(&store, None).contains(&"orphan".to_string()));
}

// =============================================================================
// Moves and group deletion
// =============================================================================

#[test]
fn moving_a_tag_into_a_group_keeps_each_projects_existing_value() {
    let store = store();
    let p1 = project(&store, "p1");
    let p2 = project(&store, "p2");
    let status = group(&store, "Status", &["Done"]);
    let done = scope_tag_id(&store, &status.id, "Done");
    let urgent = tag(&store, "urgent", None).id;
    store
        .set_project_tag_assigned(&p1.id, &urgent, true)
        .unwrap();
    store
        .set_project_tag_assigned(&p2.id, &urgent, true)
        .unwrap();
    store
        .set_project_group_tag(&p1.id, &status.id, Some(&done))
        .unwrap();

    store.move_tag(&urgent, Some(&status.id), 0).unwrap();

    // p1 already had a Status, so it keeps Done and loses urgent; p2 keeps
    // urgent, now as its Status.
    assert_eq!(tag_ids_of(&store, &p1.id), vec![done]);
    assert_eq!(tag_ids_of(&store, &p2.id), vec![urgent]);
    assert_eq!(scope_names(&store, Some(&status.id)), ["urgent", "Done"]);
    assert!(scope_names(&store, None).is_empty());
}

#[test]
fn moving_a_tag_onto_a_taken_name_is_an_error() {
    let store = store();
    let status = group(&store, "Status", &["Done"]);
    let p = project(&store, "p");
    let loose = tag(&store, "done", None).id;
    store.set_project_tag_assigned(&p.id, &loose, true).unwrap();

    assert_eq!(
        err(store.move_tag(&loose, Some(&status.id), 0)),
        "A tag named “done” already exists in Status"
    );
    // Nothing moved, and the assignment survived the rollback.
    assert_eq!(scope_names(&store, None), ["done"]);
    assert_eq!(scope_names(&store, Some(&status.id)), ["Done"]);
    assert_eq!(tag_ids_of(&store, &p.id), vec![loose]);
}

#[test]
fn ungrouping_renames_colliding_tags_and_keeps_assignments() {
    let store = store();
    let p = project(&store, "p");
    tag(&store, "High", None);
    tag(&store, "High (Priority)", None);
    let priority = group(&store, "Priority", &["High", "Medium", "Low"]);
    let high = scope_tag_id(&store, &priority.id, "High");
    store
        .set_project_group_tag(&p.id, &priority.id, Some(&high))
        .unwrap();

    store
        .delete_tag_group(&priority.id, DeleteTagGroupMode::Ungroup)
        .unwrap();

    assert_eq!(
        scope_names(&store, None),
        [
            "High",
            "High (Priority)",
            "High (Priority) 2",
            "Medium",
            "Low"
        ]
    );
    assert_eq!(tag_ids_of(&store, &p.id), vec![high]);
    assert!(store.get_tag_state().unwrap().groups.is_empty());
}

#[test]
fn deleting_a_group_with_its_tags_cascades_their_assignments() {
    let store = store();
    let p = project(&store, "p");
    let status = group(&store, "Status", &["Done"]);
    let done = scope_tag_id(&store, &status.id, "Done");
    let keep = tag(&store, "keep", None).id;
    store
        .set_project_group_tag(&p.id, &status.id, Some(&done))
        .unwrap();
    store.set_project_tag_assigned(&p.id, &keep, true).unwrap();

    store
        .delete_tag_group(&status.id, DeleteTagGroupMode::DeleteTags)
        .unwrap();

    let state = store.get_tag_state().unwrap();
    assert!(state.groups.is_empty());
    assert_eq!(
        state.tags.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        vec![keep.clone()]
    );
    assert_eq!(tag_ids_of(&store, &p.id), vec![keep]);
}

#[test]
fn deleting_a_tag_cascades_its_assignments() {
    let store = store();
    let p1 = project(&store, "p1");
    let p2 = project(&store, "p2");
    let gone = tag(&store, "gone", None).id;
    let kept = tag(&store, "kept", None).id;
    for p in [&p1, &p2] {
        store.set_project_tag_assigned(&p.id, &gone, true).unwrap();
    }
    store.set_project_tag_assigned(&p1.id, &kept, true).unwrap();

    store.delete_tag(&gone).unwrap();

    assert_eq!(tag_ids_of(&store, &p1.id), vec![kept]);
    assert!(tag_ids_of(&store, &p2.id).is_empty());
}

// =============================================================================
// Icon group
// =============================================================================

fn icon_groups(store: &Store) -> Vec<String> {
    store
        .get_tag_state()
        .unwrap()
        .groups
        .into_iter()
        .filter(|g| g.is_icon_group)
        .map(|g| g.name)
        .collect()
}

#[test]
fn a_group_created_when_none_drives_the_icon_does() {
    let store = store();
    assert!(icon_groups(&store).is_empty());

    // Into an empty catalog: the board's empty-state templates land here.
    let a = group(&store, "A", &["x"]);
    assert!(a.is_icon_group);
    assert_eq!(icon_groups(&store), ["A"]);

    // Once a group drives the icon, later groups don't take it over.
    let b = group(&store, "B", &[]);
    assert!(!b.is_icon_group);
    assert_eq!(icon_groups(&store), ["A"]);

    // With the icon group turned off, the next group picks it up again.
    store.set_icon_tag_group(None).unwrap();
    let c = group(&store, "C", &[]);
    assert!(c.is_icon_group);
    assert_eq!(icon_groups(&store), ["C"]);

    // Deleting the icon group leaves none; the next group becomes it.
    store
        .delete_tag_group(&c.id, DeleteTagGroupMode::DeleteTags)
        .unwrap();
    assert!(icon_groups(&store).is_empty());
    let d = group(&store, "D", &[]);
    assert!(d.is_icon_group);
    assert_eq!(icon_groups(&store), ["D"]);
}

#[test]
fn at_most_one_group_drives_the_icon() {
    let store = store();
    let a = group(&store, "A", &[]);
    let b = group(&store, "B", &[]);

    store.set_icon_tag_group(Some(&a.id)).unwrap();
    assert_eq!(icon_groups(&store), ["A"]);
    store.set_icon_tag_group(Some(&b.id)).unwrap();
    assert_eq!(icon_groups(&store), ["B"]);
    store.set_icon_tag_group(None).unwrap();
    assert!(icon_groups(&store).is_empty());

    store.set_icon_tag_group(Some(&a.id)).unwrap();
    assert_eq!(
        err(store.set_icon_tag_group(Some("nope"))),
        "Tag group not found: nope"
    );
    assert_eq!(icon_groups(&store), ["A"]);
}

fn sidebar_groups(store: &Store) -> Vec<String> {
    store
        .get_tag_state()
        .unwrap()
        .groups
        .into_iter()
        .filter(|g| g.show_in_sidebar)
        .map(|g| g.name)
        .collect()
}

#[test]
fn any_number_of_groups_can_pin_their_board_to_the_sidebar() {
    let (store, mut rx) = store_with_feed();
    let a = group(&store, "A", &["x"]);
    let b = group(&store, "B", &[]);
    // Off by default, for new groups and (0046) for existing ones.
    assert!(!a.show_in_sidebar && !b.show_in_sidebar);
    assert!(sidebar_groups(&store).is_empty());
    drain(&mut rx);

    store.set_tag_group_sidebar(&a.id, true).unwrap();
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);
    store.set_tag_group_sidebar(&b.id, true).unwrap();
    assert_eq!(sidebar_groups(&store), ["A", "B"]);

    // Pinning a pinned group changes nothing and publishes nothing.
    store.set_tag_group_sidebar(&a.id, true).unwrap();
    drain(&mut rx);
    store.set_tag_group_sidebar(&a.id, true).unwrap();
    assert_eq!(drain(&mut rx), vec![]);

    store.set_tag_group_sidebar(&a.id, false).unwrap();
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);
    assert_eq!(sidebar_groups(&store), ["B"]);

    // The flag is the group's own: the icon group is unaffected either way.
    assert_eq!(icon_groups(&store), ["A"]);

    assert_eq!(
        err(store.set_tag_group_sidebar("nope", true)),
        "Tag group not found: nope"
    );
    // Deleting a pinned group takes its sidebar row with it.
    store
        .delete_tag_group(&b.id, DeleteTagGroupMode::Ungroup)
        .unwrap();
    assert!(sidebar_groups(&store).is_empty());
}

// =============================================================================
// Errors, state shape and the change feed
// =============================================================================

#[test]
fn unknown_ids_are_reported() {
    let store = store();
    let p = project(&store, "p");
    assert_eq!(err(store.delete_tag("t?")), "Tag not found: t?");
    assert_eq!(
        err(store.update_tag("t?", "x", TagColor::Red, None)),
        "Tag not found: t?"
    );
    assert_eq!(err(store.move_tag("t?", None, 0)), "Tag not found: t?");
    assert_eq!(
        err(store.set_project_tag_assigned(&p.id, "t?", true)),
        "Tag not found: t?"
    );
    assert_eq!(
        err(store.rename_tag_group("g?", "x")),
        "Tag group not found: g?"
    );
    assert_eq!(
        err(store.delete_tag_group("g?", DeleteTagGroupMode::Ungroup)),
        "Tag group not found: g?"
    );
    assert_eq!(
        err(store.create_tag(&named("x"), Some("g?"), None)),
        "Tag group not found: g?"
    );
    assert_eq!(
        err(store.set_project_group_tag(&p.id, "g?", None)),
        "Tag group not found: g?"
    );
    let t = tag(&store, "t", None);
    assert_eq!(
        err(store.move_tag(&t.id, Some("g?"), 0)),
        "Tag group not found: g?"
    );
}

#[test]
fn tag_state_is_sorted_and_serializes_camel_case() {
    let store = store();
    let b = project(&store, "b");
    let a = project(&store, "a");
    let status = group(&store, "Status", &["Done"]);
    store.set_icon_tag_group(Some(&status.id)).unwrap();
    let done = scope_tag_id(&store, &status.id, "Done");
    let loose = tag(&store, "loose", None).id;
    for p in [&b, &a] {
        store.set_project_tag_assigned(&p.id, &done, true).unwrap();
        store.set_project_tag_assigned(&p.id, &loose, true).unwrap();
    }

    let state = store.get_tag_state().unwrap();
    let pairs: Vec<(String, String)> = state
        .assignments
        .iter()
        .map(|a| (a.project_id.clone(), a.tag_id.clone()))
        .collect();
    let mut expected = pairs.clone();
    expected.sort();
    assert_eq!(pairs, expected);
    assert_eq!(pairs.len(), 4);

    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["groups"][0]["isIconGroup"], json!(true));
    let done_json = json["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(done))
        .unwrap();
    assert_eq!(done_json["groupId"], json!(status.id));
    assert!(done_json["color"].is_string());
    assert_eq!(done_json["icon"], json!(null));
    assert!(json["assignments"][0]["projectId"].is_string());
    assert!(json["assignments"][0].get("sortOrder").is_none());
}

#[test]
fn wire_enums_use_the_command_spellings() {
    assert_eq!(
        serde_json::from_value::<DeleteTagGroupMode>(json!("ungroup")).unwrap(),
        DeleteTagGroupMode::Ungroup
    );
    assert_eq!(
        serde_json::from_value::<DeleteTagGroupMode>(json!("deleteTags")).unwrap(),
        DeleteTagGroupMode::DeleteTags
    );
    for color in TagColor::AUTO_ORDER {
        assert_eq!(serde_json::to_value(color).unwrap(), json!(color.as_str()));
        assert_eq!(color.as_str().parse::<TagColor>().unwrap(), color);
    }
    let new: NewTag = serde_json::from_value(json!({ "name": "q4" })).unwrap();
    assert_eq!((new.color, new.icon), (None, None));
}

#[test]
fn writes_publish_tags_once_and_no_op_writes_publish_nothing() {
    let (store, mut rx) = store_with_feed();
    let p = project(&store, "p");
    let a = group(&store, "A", &["one", "two"]);
    let b = group(&store, "B", &[]);
    store.set_icon_tag_group(Some(&a.id)).unwrap();
    let one = scope_tag_id(&store, &a.id, "one");
    let loose = tag(&store, "loose", None);
    store
        .set_project_group_tag(&p.id, &a.id, Some(&one))
        .unwrap();
    store
        .set_project_tag_assigned(&p.id, &loose.id, true)
        .unwrap();
    drain(&mut rx);

    // Each of these leaves every row as it was.
    store.rename_tag_group(&a.id, " A ").unwrap();
    store.set_icon_tag_group(Some(&a.id)).unwrap();
    store
        .reorder_tag_groups(&[a.id.clone(), b.id.clone()])
        .unwrap();
    store
        .update_tag(&loose.id, "loose", loose.color, None)
        .unwrap();
    store.move_tag(&one, Some(&a.id), 0).unwrap();
    store.move_tag(&loose.id, None, 5).unwrap();
    store
        .set_project_group_tag(&p.id, &a.id, Some(&one))
        .unwrap();
    store.set_project_group_tag(&p.id, &b.id, None).unwrap();
    store
        .set_project_tag_assigned(&p.id, &loose.id, true)
        .unwrap();
    store.set_project_tag_assigned(&p.id, &one, true).unwrap();
    let other = project(&store, "other");
    drain(&mut rx);
    store
        .set_project_tag_assigned(&other.id, &loose.id, false)
        .unwrap();
    store.set_icon_tag_group(Some(&a.id)).unwrap();
    assert_eq!(drain(&mut rx), vec![]);

    // A real change publishes exactly one `Tags`, never `Project`.
    store.set_icon_tag_group(None).unwrap();
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);
    store.set_icon_tag_group(None).unwrap();
    assert_eq!(drain(&mut rx), vec![]);
    store
        .update_tag(&loose.id, "Loose", loose.color, None)
        .unwrap();
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);
    store.set_project_group_tag(&p.id, &a.id, None).unwrap();
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);

    // A failed write publishes nothing.
    assert!(store
        .reorder_tag_groups(std::slice::from_ref(&a.id))
        .is_err());
    assert!(store.create_tag(&named("loose"), None, None).is_err());
    assert_eq!(drain(&mut rx), vec![]);
}

#[test]
fn deleting_a_project_cascades_its_tags_and_publishes_tags() {
    let (store, mut rx) = store_with_feed();
    let p = project(&store, "p");
    let keep = project(&store, "keep");
    let t = tag(&store, "t", None).id;
    store.set_project_tag_assigned(&p.id, &t, true).unwrap();
    store.set_project_tag_assigned(&keep.id, &t, true).unwrap();
    drain(&mut rx);

    store.delete_project(&p.id).unwrap();

    assert_eq!(
        drain(&mut rx),
        vec![
            StoreChange::Project {
                project_id: Some(p.id.clone())
            },
            StoreChange::Tags,
        ]
    );
    let state = store.get_tag_state().unwrap();
    assert_eq!(
        state.assignments,
        vec![ProjectTag {
            project_id: keep.id.clone(),
            tag_id: t.clone(),
        }]
    );
    assert_eq!(state.tags.len(), 1);
}

// =============================================================================
// Legacy status options parser
// =============================================================================

fn option(id: &str, label: &str, icon: &str, color: TagColor) -> LegacyStatusOption {
    LegacyStatusOption {
        id: id.to_string(),
        label: label.to_string(),
        icon: icon.to_string(),
        color,
    }
}

fn parse(prefs: serde_json::Value) -> LegacyStatusOptions {
    parse_legacy_status_options(Some(&prefs))
}

#[test]
fn a_missing_or_unusable_option_list_parses_as_the_defaults() {
    assert_eq!(
        parse_legacy_status_options(None),
        LegacyStatusOptions::Defaults
    );
    assert_eq!(parse(json!({})), LegacyStatusOptions::Defaults);
    for value in [
        json!(null),
        json!("idea"),
        json!({ "id": "idea" }),
        json!(3),
    ] {
        assert_eq!(
            parse(json!({ LEGACY_STATUS_OPTIONS_KEY: value })),
            LegacyStatusOptions::Defaults
        );
    }
    // A list where nothing survives is unusable too.
    assert_eq!(
        parse(json!({ LEGACY_STATUS_OPTIONS_KEY: [null, 1, "x", { "label": "no id" }] })),
        LegacyStatusOptions::Defaults
    );
}

#[test]
fn an_empty_option_list_is_respected() {
    assert_eq!(
        parse(json!({ LEGACY_STATUS_OPTIONS_KEY: [] })),
        LegacyStatusOptions::Custom(vec![])
    );
}

#[test]
fn option_entries_are_validated_like_the_frontend_did() {
    let parsed = parse(json!({ LEGACY_STATUS_OPTIONS_KEY: [
        { "id": "a", "label": "Alpha", "icon": "rocket", "color": "red" },
        "not an object",
        ["nor", "this"],
        { "id": "", "label": "empty id" },
        { "id": 7, "label": "numeric id" },
        { "id": "no-label" },
        { "id": "b", "label": 5 },
        // `b` was skipped above for its label, so this one is its first.
        { "id": "b", "label": "", "color": "magenta" },
        { "id": "a", "label": "duplicate id", "color": "blue" },
        { "id": "c", "label": "No icon" },
        { "id": "d", "label": "Empty icon", "icon": "", "color": "cyan" },
        { "id": "e", "label": "Bad icon", "icon": "Not Kebab", "color": "green" },
    ] }));
    assert_eq!(
        parsed,
        LegacyStatusOptions::Custom(vec![
            option("a", "Alpha", "rocket", TagColor::Red),
            option("b", "", "circle-dot", TagColor::Gray),
            option("c", "No icon", "circle-dot", TagColor::Gray),
            option("d", "Empty icon", "circle-dot", TagColor::Cyan),
            option("e", "Bad icon", "circle-dot", TagColor::Green),
        ])
    );
}

#[test]
fn the_frozen_defaults_are_the_seven_built_ins() {
    let ids: Vec<String> = LegacyStatusOptions::Defaults
        .options()
        .into_iter()
        .map(|o| o.id)
        .collect();
    assert_eq!(
        ids,
        [
            "idea",
            "in-progress",
            "polishing",
            "needs-review",
            "done",
            "on-hold",
            "blocked"
        ]
    );
}

// =============================================================================
// Legacy import
// =============================================================================

/// Projects named after their (possibly missing) legacy selection. The
/// frozen `status_override` column is off the `Project` model, so it's
/// written directly, the way pre-tags builds left it.
fn projects_with_overrides(store: &Store, overrides: &[Option<&str>]) -> Vec<Project> {
    overrides
        .iter()
        .enumerate()
        .map(|(i, selection)| {
            let p = project(store, &format!("p{i}"));
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE projects SET status_override = ?1 WHERE id = ?2",
                    rusqlite::params![selection, p.id],
                )
                .unwrap();
            p
        })
        .collect()
}

fn frozen_status_override(store: &Store, project_id: &str) -> Option<String> {
    store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT status_override FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn status_tags(store: &Store) -> Vec<Tag> {
    let mut tags: Vec<Tag> = store
        .get_tag_state()
        .unwrap()
        .tags
        .into_iter()
        .filter(|t| t.group_id.as_deref() == Some(LEGACY_STATUS_GROUP_ID))
        .collect();
    tags.sort_by_key(|t| t.position);
    tags
}

#[test]
fn importing_the_defaults_seeds_status_and_copies_selections() {
    let (store, mut rx) = store_with_feed();
    let ps = projects_with_overrides(&store, &[Some("done"), Some("blocked"), None]);
    drain(&mut rx);

    let outcome = store
        .import_legacy_project_statuses(&LegacyStatusOptions::Defaults)
        .unwrap();

    assert_eq!(
        outcome,
        LegacyImportOutcome::Imported {
            tags: 7,
            assignments: 2,
            dropped_dangling: 0,
        }
    );
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);

    let state = store.get_tag_state().unwrap();
    assert_eq!(state.groups.len(), 1);
    let status = &state.groups[0];
    assert_eq!(
        (status.id.as_str(), status.name.as_str(), status.position),
        ("status", "Status", 0)
    );
    assert!(status.is_icon_group);

    let tags = status_tags(&store);
    let expected: Vec<(String, String, Option<String>, TagColor, i64)> = LEGACY_DEFAULT_STATUSES
        .iter()
        .enumerate()
        .map(|(i, (id, label, icon, color))| {
            (
                id.to_string(),
                label.to_string(),
                Some(icon.to_string()),
                *color,
                i as i64,
            )
        })
        .collect();
    let actual: Vec<(String, String, Option<String>, TagColor, i64)> = tags
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                t.name.clone(),
                t.icon.clone(),
                t.color,
                t.position,
            )
        })
        .collect();
    assert_eq!(actual, expected);

    assert_eq!(tag_ids_of(&store, &ps[0].id), vec!["done"]);
    assert_eq!(tag_ids_of(&store, &ps[1].id), vec!["blocked"]);
    assert!(tag_ids_of(&store, &ps[2].id).is_empty());
}

#[test]
fn importing_a_custom_list_keeps_ids_and_order_and_drops_dangling_selections() {
    let store = store();
    let first = "8d0c6a1e-6f0b-4a4e-9a51-3c1f1f0f2a01";
    let second = "1b7e2f9c-0d4a-4c1e-8f3b-7a6d5e4c3b02";
    let ps = projects_with_overrides(
        &store,
        &[
            Some(second),
            Some("idea"),
            Some(first),
            Some("deleted-option"),
        ],
    );
    let options = parse(json!({ LEGACY_STATUS_OPTIONS_KEY: [
        { "id": second, "label": "Shipping", "icon": "rocket", "color": "green" },
        { "id": first, "label": "Thinking", "icon": "brain", "color": "purple" },
    ] }));

    let outcome = store.import_legacy_project_statuses(&options).unwrap();

    // `idea` was a built-in, but this user's list no longer had it: it
    // already showed Default, and now has no Status tag.
    assert_eq!(
        outcome,
        LegacyImportOutcome::Imported {
            tags: 2,
            assignments: 2,
            dropped_dangling: 2,
        }
    );
    let tags = status_tags(&store);
    assert_eq!(
        tags.iter()
            .map(|t| (t.id.as_str(), t.name.as_str(), t.color))
            .collect::<Vec<_>>(),
        vec![
            (second, "Shipping", TagColor::Green),
            (first, "Thinking", TagColor::Purple),
        ]
    );
    assert_eq!(tag_ids_of(&store, &ps[0].id), vec![second]);
    assert!(tag_ids_of(&store, &ps[1].id).is_empty());
    assert_eq!(tag_ids_of(&store, &ps[2].id), vec![first]);
    assert!(tag_ids_of(&store, &ps[3].id).is_empty());
}

#[test]
fn importing_blank_repeated_and_long_labels_makes_unique_names() {
    let store = store();
    let long = "y".repeat(70);
    let options = LegacyStatusOptions::Custom(vec![
        option("a", "", "circle-dot", TagColor::Gray),
        option("b", "   ", "circle-dot", TagColor::Gray),
        option("c", "Done", "circle-check", TagColor::Green),
        option("d", "done", "circle-check", TagColor::Green),
        option("e", " Done ", "circle-check", TagColor::Green),
        option("f", &long, "circle-dot", TagColor::Gray),
        option("g", &long, "circle-dot", TagColor::Gray),
    ]);

    store.import_legacy_project_statuses(&options).unwrap();

    let names: Vec<String> = status_tags(&store).into_iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        vec![
            "Untitled status".to_string(),
            "Untitled status 2".to_string(),
            "Done".to_string(),
            "done 2".to_string(),
            "Done 3".to_string(),
            "y".repeat(64),
            format!("{} 2", "y".repeat(62)),
        ]
    );
}

#[test]
fn importing_an_empty_list_creates_an_empty_status_group() {
    let store = store();
    let ps = projects_with_overrides(&store, &[Some("done")]);

    let outcome = store
        .import_legacy_project_statuses(&LegacyStatusOptions::Custom(vec![]))
        .unwrap();

    assert_eq!(
        outcome,
        LegacyImportOutcome::Imported {
            tags: 0,
            assignments: 0,
            dropped_dangling: 1,
        }
    );
    let state = store.get_tag_state().unwrap();
    assert_eq!(state.groups.len(), 1);
    assert!(state.tags.is_empty());
    assert!(tag_ids_of(&store, &ps[0].id).is_empty());
}

#[test]
fn importing_into_a_catalog_built_during_a_held_import_appends_a_numbered_group() {
    // The import was held (preferences.json unreadable) and meanwhile the
    // user built a Status group from the board's empty state, which made it
    // the icon group. The retried import must not collide with it.
    let (store, mut rx) = store_with_feed();
    let ps = projects_with_overrides(&store, &[Some("done"), Some("blocked"), None]);
    let theirs = group(&store, "Status", &["Todo", "Done"]);
    store
        .set_project_group_tag(
            &ps[2].id,
            &theirs.id,
            Some(&scope_tag_id(&store, &theirs.id, "Todo")),
        )
        .unwrap();
    drain(&mut rx);

    let outcome = store
        .import_legacy_project_statuses(&LegacyStatusOptions::Defaults)
        .unwrap();

    assert_eq!(
        outcome,
        LegacyImportOutcome::Imported {
            tags: 7,
            assignments: 2,
            dropped_dangling: 0,
        }
    );
    assert_eq!(drain(&mut rx), vec![StoreChange::Tags]);

    // Theirs stays first and keeps the icon; the legacy group comes after it
    // under a free name.
    assert_eq!(group_names(&store), vec!["Status", "Status 2"]);
    assert_eq!(icon_groups(&store), vec!["Status"]);
    let state = store.get_tag_state().unwrap();
    let legacy = state
        .groups
        .iter()
        .find(|g| g.id == LEGACY_STATUS_GROUP_ID)
        .unwrap();
    assert_eq!((legacy.name.as_str(), legacy.position), ("Status 2", 1));
    assert!(!legacy.is_icon_group);

    // Option ids are still the tag ids, so the selections copy across, and
    // the user's own assignment is untouched. Each scope keeps its own
    // "Done".
    assert_eq!(
        status_tags(&store)
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        LEGACY_DEFAULT_STATUSES
            .iter()
            .map(|(id, ..)| *id)
            .collect::<Vec<_>>()
    );
    assert_eq!(tag_ids_of(&store, &ps[0].id), vec!["done"]);
    assert_eq!(tag_ids_of(&store, &ps[1].id), vec!["blocked"]);
    assert_eq!(
        tag_ids_of(&store, &ps[2].id),
        vec![scope_tag_id(&store, &theirs.id, "Todo")]
    );
    assert_eq!(scope_names(&store, Some(&theirs.id)), vec!["Todo", "Done"]);

    // The marker went with it: the next launch doesn't import again.
    assert_eq!(
        store
            .import_legacy_project_statuses(&LegacyStatusOptions::Defaults)
            .unwrap(),
        LegacyImportOutcome::AlreadyImported
    );
}

#[test]
fn importing_into_a_catalog_with_no_icon_group_makes_the_legacy_group_drive_the_icon() {
    let store = store();
    let theirs = group(&store, "Priority", &["P0", "P1"]);
    store.set_icon_tag_group(None).unwrap();
    assert!(icon_groups(&store).is_empty());

    store
        .import_legacy_project_statuses(&LegacyStatusOptions::Defaults)
        .unwrap();

    assert_eq!(group_names(&store), vec!["Priority", "Status"]);
    assert_eq!(icon_groups(&store), vec!["Status"]);
    assert_eq!(scope_names(&store, Some(&theirs.id)), vec!["P0", "P1"]);
}

#[test]
fn a_second_import_is_a_no_op_and_status_override_stays_frozen() {
    let (store, mut rx) = store_with_feed();
    let ps = projects_with_overrides(&store, &[Some("done"), Some("gone")]);
    store
        .import_legacy_project_statuses(&LegacyStatusOptions::Defaults)
        .unwrap();
    let before = store.get_tag_state().unwrap();
    drain(&mut rx);

    let custom = LegacyStatusOptions::Custom(vec![option("x", "X", "x", TagColor::Red)]);
    assert_eq!(
        store.import_legacy_project_statuses(&custom).unwrap(),
        LegacyImportOutcome::AlreadyImported
    );
    assert_eq!(drain(&mut rx), vec![]);

    let after = store.get_tag_state().unwrap();
    assert_eq!(after.tags.len(), before.tags.len());
    assert_eq!(after.assignments, before.assignments);
    let overrides: Vec<Option<String>> = ps
        .iter()
        .map(|p| frozen_status_override(&store, &p.id))
        .collect();
    assert_eq!(
        overrides,
        vec![Some("done".to_string()), Some("gone".to_string())]
    );
}
