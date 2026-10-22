use super::*;

#[test]
fn valid_selection_resolves_every_canonical_wire_name() {
    for name in ["unflagged", "picked", "rejected"] {
        let expected = valid_selection_state(name).expect("canonical name");
        assert_eq!(
            valid_selection(&Value::String(name.to_owned())),
            Some(expected)
        );
    }
}

#[test]
fn valid_selection_rejects_non_strings_and_unknown_names() {
    assert_eq!(valid_selection(&Value::Null), None);
    assert_eq!(valid_selection(&Value::Number(1.into())), None);
    assert_eq!(valid_selection(&Value::Bool(true)), None);
    for name in ["", "selected", "undecided", "Picked"] {
        assert_eq!(
            valid_selection(&Value::String(name.to_owned())),
            None,
            "{name}"
        );
    }
}

#[test]
fn batch_admission_rejects_unflagged_and_admits_decision_states() {
    assert_eq!(
        valid_batch_selection(&Value::String("unflagged".into())),
        None
    );
    assert_eq!(
        valid_batch_selection(&Value::String("picked".into())),
        Some(SelectionState::Picked)
    );
    assert_eq!(
        valid_batch_selection(&Value::String("rejected".into())),
        Some(SelectionState::Rejected)
    );
    assert_eq!(
        valid_batch_selection(&Value::String("selected".into())),
        None
    );
}
