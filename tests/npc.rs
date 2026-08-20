use npc_system::belief::{Belief, BeliefKind};
use npc_system::goal::{Goal, GoalKind};
use npc_system::id::{NpcId, TownId};
use npc_system::npc::{Attributes, Npc, Sex};
use npc_system::relationship::{Relationship, RelationshipKind};
use npc_system::utility::{Action, PersonCandidate, Situation, choose_action, score_action};

fn test_npc(id: NpcId, beliefs: Vec<Belief>) -> Npc {
    Npc::new(
        id,
        Npc::default_name(id),
        30,
        Sex::Female,
        TownId(0),
        TownId(0),
        Attributes::uniform(5),
        beliefs,
        Goal::new(GoalKind::LivePeacefully, 0),
    )
}

#[test]
fn test_a_one_npc_helps_family_when_family_belief_and_relation_are_strong() {
    let actor_id = NpcId(0);
    let family_id = NpcId(1);
    let mut npc = test_npc(
        actor_id,
        vec![
            Belief::new(BeliefKind::ProtectFamily, 9),
            Belief::new(BeliefKind::KeepPromises, 5),
        ],
    );
    assert!(npc.add_child(family_id));
    assert!(npc.set_relationship(
        family_id,
        Relationship::with_relation(9, 9, RelationshipKind::Family, 0, 1),
    ));

    let family = PersonCandidate::from_relationship(
        family_id,
        npc.relationship(family_id)
            .expect("家族への関係が設定されている"),
    )
    .needing_help(3);
    let situation = Situation {
        danger: 3,
        people: vec![family],
        ..Situation::default()
    };

    assert_eq!(
        choose_action(&npc, &situation),
        Some(Action::HelpPerson(family_id))
    );
}

#[test]
fn test_b_help_others_belief_overcomes_low_danger_but_not_extreme_danger() {
    let target = NpcId(1);
    let altruistic = test_npc(
        NpcId(0),
        vec![
            Belief::new(BeliefKind::HelpOthers, 9),
            Belief::new(BeliefKind::JudgeIndividuals, 5),
        ],
    );
    let low_danger = Situation {
        danger: 3,
        people: vec![PersonCandidate::new(target, 8).needing_help(3)],
        ..Situation::default()
    };

    assert_eq!(
        choose_action(&altruistic, &low_danger),
        Some(Action::HelpPerson(target)),
        "HelpOthers=9、relation=8、danger=3 なら助ける"
    );
    assert!(
        score_action(&altruistic, &low_danger, Action::HelpPerson(target))
            > score_action(&altruistic, &low_danger, Action::Rest),
        "低危険時には援助のUtilityが休息を上回る"
    );

    let reluctant = test_npc(
        NpcId(0),
        vec![
            Belief::new(BeliefKind::HelpOthers, 1),
            Belief::new(BeliefKind::ValueOrder, 5),
        ],
    );
    let extreme_danger = Situation {
        danger: 10,
        people: vec![PersonCandidate::new(target, 2).needing_help(10)],
        ..Situation::default()
    };

    assert_ne!(
        choose_action(&reluctant, &extreme_danger),
        Some(Action::HelpPerson(target)),
        "HelpOthers=1、relation=2、danger=10 なら助けない"
    );
    assert!(
        score_action(&reluctant, &extreme_danger, Action::HelpPerson(target))
            < score_action(&reluctant, &extreme_danger, Action::Rest),
        "極端な危険下では援助のUtilityが安全な行動を下回る"
    );
}

#[test]
fn npc_numeric_domains_cover_every_value_from_zero_through_ten() {
    for value in 0..=10 {
        let attributes = Attributes::uniform(value);
        assert_eq!(
            [
                attributes.physical,
                attributes.dexterity,
                attributes.intelligence,
                attributes.charisma,
                attributes.willpower,
            ],
            [value; 5]
        );
        assert!(attributes.values_in_range());

        let belief = Belief::new(BeliefKind::HelpOthers, value);
        assert_eq!(belief.strength, value);
        assert!(belief.is_valid());
    }

    assert_eq!(Attributes::uniform(u8::MAX), Attributes::uniform(10));
    assert_eq!(Belief::new(BeliefKind::HelpOthers, u8::MAX).strength, 10);
}
