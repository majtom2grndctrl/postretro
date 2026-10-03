use super::*;
use crate::{
    ActivationCharge, ActivationStepDescriptor, ActivationTrigger, IrNode, IrValue, NumberOrIr,
    ShotScaleDescriptor, WeaponActivationDescriptor,
};
fn input() -> IrNode {
    IrNode::Input {
        name: "charge".into(),
        owner: None,
    }
}
fn curve(multiplier: f32) -> NumberOrIr {
    NumberOrIr::Ir(IrNode::Add {
        a: Box::new(IrNode::Mul {
            a: Box::new(input()),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(multiplier - 1.0),
            }),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(1.0),
        }),
    })
}
fn action(multiplier: f32) -> WeaponActivationDescriptor {
    let mut result = WeaponActivationDescriptor::single(ActivationTrigger::Press, 400.0);
    result.charge = Some(ActivationCharge {
        min_ms: 200.0,
        full_ms: 1000.0,
    });
    result.steps = vec![ActivationStepDescriptor::Shot {
        scale: Box::new(ShotScaleDescriptor {
            damage: curve(multiplier),
            ..Default::default()
        }),
    }];
    result
}
fn base() -> ShotScaleInputs {
    ShotScaleInputs {
        damage: 10.0,
        range: 96.0,
        projectile_speed: Some(40.0),
        projectile_radius: Some(0.5),
        projectile_size: Some(1.5),
        knockback_speed: Some(3.0),
        splash_knockback_speed: None,
        resource_cost: ShotResourceCost::Cell(5.0),
    }
}
#[test]
fn authored_damage_expression_changes_three_to_six_without_cost_or_visual_change() {
    for multiplier in [3.0, 6.0] {
        let compiled = CompiledActivation::compile(&action(multiplier), "fixture.primary").unwrap();
        for charge in [0.0, 0.2, 0.5, 1.0] {
            let values = compiled
                .resolve_scales(0, charge)
                .unwrap()
                .apply(base())
                .unwrap();
            assert!((values.damage - 10.0 * (1.0 + charge * (multiplier - 1.0))).abs() < 0.0001);
            assert_eq!(values.resource_cost, ShotResourceCost::Cell(5.0));
            assert_eq!(values.projectile_size, Some(1.5));
            assert_eq!(values.projectile_radius, Some(0.5));
        }
    }
}
#[test]
fn authoring_quantizes_waits_and_enforces_program_and_charge_limits() {
    let mut descriptor = action(3.0);
    descriptor.steps = vec![
        ActivationStepDescriptor::Shot {
            scale: Default::default(),
        },
        ActivationStepDescriptor::Wait { duration_ms: 80.0 },
        ActivationStepDescriptor::Shot {
            scale: Default::default(),
        },
    ];
    let compiled = CompiledActivation::compile(&descriptor, "fixture").unwrap();
    assert_eq!(compiled.timing.steps[1], ActivationStep::Wait { ticks: 5 });
    assert_eq!(
        compiled.timing.charge,
        Some(ChargeTiming {
            min_ticks: 12,
            full_ticks: 60
        })
    );
    descriptor.trigger = ActivationTrigger::Hold;
    assert!(descriptor.validate("fixture").is_err());
    descriptor.trigger = ActivationTrigger::Press;
    descriptor.steps.insert(
        1,
        ActivationStepDescriptor::Shot {
            scale: Default::default(),
        },
    );
    assert!(descriptor.validate("fixture").is_err());
    descriptor.steps = vec![
        ActivationStepDescriptor::Shot {
            scale: Default::default(),
        },
        ActivationStepDescriptor::Wait {
            duration_ms: 59999.0,
        },
        ActivationStepDescriptor::Wait { duration_ms: 1.0 },
        ActivationStepDescriptor::Shot {
            scale: Default::default(),
        },
    ];
    assert!(
        descriptor.validate("fixture").is_err(),
        "total quantized waits exceed 60 seconds"
    );
}
#[test]
fn authoring_rejects_store_owned_boolean_and_overbudget_ir() {
    let mut descriptor = action(3.0);
    for node in [
        IrNode::Input {
            name: "player.health".into(),
            owner: None,
        },
        IrNode::Input {
            name: "charge".into(),
            owner: Some("@impact.source".into()),
        },
        IrNode::Const {
            value: IrValue::Bool(true),
        },
        IrNode::Const {
            value: IrValue::Number(f32::INFINITY),
        },
    ] {
        descriptor.steps = vec![ActivationStepDescriptor::Shot {
            scale: Box::new(ShotScaleDescriptor {
                damage: NumberOrIr::Ir(node),
                ..Default::default()
            }),
        }];
        assert!(descriptor.validate("fixture").is_err());
    }
    let mut node = input();
    for _ in 0..MAX_ACTIVATION_IR_DEPTH {
        node = IrNode::Add {
            a: Box::new(node),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        };
    }
    descriptor.steps = vec![ActivationStepDescriptor::Shot {
        scale: Box::new(ShotScaleDescriptor {
            damage: NumberOrIr::Ir(node),
            ..Default::default()
        }),
    }];
    assert!(descriptor.validate("fixture").is_err());
    fn tree(depth: usize) -> IrNode {
        if depth == 0 {
            input()
        } else {
            IrNode::Add {
                a: Box::new(tree(depth - 1)),
                b: Box::new(tree(depth - 1)),
            }
        }
    }
    descriptor.steps = vec![ActivationStepDescriptor::Shot {
        scale: Box::new(ShotScaleDescriptor {
            damage: NumberOrIr::Ir(tree(8)),
            ..Default::default()
        }),
    }];
    assert!(descriptor.validate("fixture").is_err());
}
#[test]
fn scaled_values_reject_overflow_and_round_ammo_up_before_debit() {
    let mut descriptor = action(3.0);
    descriptor.steps = vec![ActivationStepDescriptor::Shot {
        scale: Box::new(ShotScaleDescriptor {
            resource_cost: 0.25.into(),
            ..Default::default()
        }),
    }];
    let compiled = CompiledActivation::compile(&descriptor, "fixture").unwrap();
    let scales = compiled.resolve_scales(0, 1.0).unwrap();
    let mut values = base();
    values.resource_cost = ShotResourceCost::Ammo(3);
    assert_eq!(
        scales.apply(values).unwrap().resource_cost,
        ShotResourceCost::Ammo(1)
    );
    values.resource_cost = ShotResourceCost::Ammo(u32::MAX);
    let scales = CompiledActivation::compile(&action(3.0), "fixture")
        .unwrap()
        .resolve_scales(0, 1.0)
        .unwrap();
    values.damage = f32::MAX;
    assert_eq!(scales.apply(values).unwrap_err().field, "damage");
    values.damage = 10.0;
    let mut resource_scales = scales;
    resource_scales.resource_cost = 2.0;
    assert_eq!(
        resource_scales.apply(values).unwrap_err().field,
        "resourceCost"
    );
}

#[test]
fn invalid_intermediate_arithmetic_cannot_be_hidden_by_later_addition() {
    use crate::{BakedIr, CURRENT_IR_VERSION, bind, eval_value, eval_value_checked};
    struct TestChargeScope;
    impl crate::BindingScope for TestChargeScope {
        type InputHandle = ();
        type OutputHandle = ();
        fn resolve_input(&self, name: &str) -> Option<crate::ResolvedInput<()>> {
            (name == "charge").then_some(crate::ResolvedInput {
                handle: (),
                ir_type: crate::IrType::Number,
            })
        }
        fn resolve_output(&self, _: &str) -> Option<crate::ResolvedOutput<()>> {
            None
        }
        fn read(&self, _: &()) -> IrValue {
            IrValue::Number(1.0)
        }
        fn write(&mut self, _: &(), _: IrValue) {
            unreachable!()
        }
    }
    let overflow = IrNode::Mul {
        a: Box::new(IrNode::Mul {
            a: Box::new(input()),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(3e38),
            }),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(2.0),
        }),
    };
    for node in [
        overflow.clone(),
        IrNode::Add {
            a: Box::new(overflow),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        },
    ] {
        let scope = TestChargeScope;
        let bound = bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root: node.clone(),
            },
            &scope,
        )
        .unwrap();
        assert!(
            matches!(eval_value(&bound, &scope), IrValue::Number(value) if value == 0.0 || value == 1.0)
        );
        assert!(eval_value_checked(&bound, &scope).is_err());
        for resource_axis in [false, true] {
            let mut descriptor = action(3.0);
            let mut scale = ShotScaleDescriptor::default();
            if resource_axis {
                scale.resource_cost = NumberOrIr::Ir(node.clone());
            } else {
                scale.damage = NumberOrIr::Ir(node.clone());
            }
            descriptor.steps = vec![ActivationStepDescriptor::Shot {
                scale: Box::new(scale),
            }];
            let compiled = CompiledActivation::compile(&descriptor, "fixture").unwrap();
            assert_eq!(
                compiled.resolve_scales(0, 1.0).unwrap_err().field,
                if resource_axis {
                    "resourceCost"
                } else {
                    "damage"
                }
            );
        }
    }
}

#[test]
fn uncharged_action_reads_full_charge_independently_of_cursor_input() {
    let mut descriptor = action(3.0);
    descriptor.charge = None;
    let compiled = CompiledActivation::compile(&descriptor, "fixture").unwrap();
    assert_eq!(compiled.resolve_scales(0, 0.0).unwrap().damage, 3.0);
}
