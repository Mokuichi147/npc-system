use npc_system::relationship::{Relationship, RelationshipKind};

#[test]
fn test_c_all_affinity_and_relation_inputs_stay_in_zero_through_ten() {
    for affinity in 0..=10 {
        for relation in 0..=10 {
            let relationship = Relationship::with_relation(
                affinity,
                relation,
                RelationshipKind::Acquaintance,
                12,
                6,
            );

            assert_eq!(relationship.affinity, affinity);
            assert_eq!(relationship.relation, relation);
            assert!(
                relationship.values_in_range(),
                "affinity={affinity}, relation={relation} が範囲外になった"
            );
        }
    }
}

#[test]
fn relationship_public_mutations_clamp_both_boundaries() {
    for initial in 0..=10 {
        let mut relationship =
            Relationship::with_relation(initial, initial, RelationshipKind::Friend, 0, 1);

        relationship.adjust_relation(i8::MIN);
        assert!(relationship.relation <= 10);
        relationship.adjust_relation(i8::MAX);
        assert!(relationship.relation <= 10);

        relationship.set_affinity(u8::MAX);
        relationship.set_relation(u8::MAX);
        assert_eq!(relationship.affinity, 10);
        assert_eq!(relationship.relation, 10);
        assert!(relationship.values_in_range());
    }
}
