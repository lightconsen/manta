use criterion::{black_box, criterion_group, criterion_main, Criterion};
use syscity::core::models::{CreateEntityRequest, Status, UpdateEntityRequest};
use syscity::core::Engine;

fn bench_engine_create_entity(c: &mut Criterion) {
    let engine = Engine::new();

    c.bench_function("engine_create_entity", |b| {
        let mut counter = 0u64;
        b.iter(|| {
            counter += 1;
            let request = CreateEntityRequest {
                name: format!("Entity {}", counter),
                description: Some("Benchmark entity".to_string()),
                tags: Some(vec!["bench".to_string()]),
            };
            let _ = engine.create_entity(black_box(request)).unwrap();
        })
    });
}

fn bench_engine_get_entity(c: &mut Criterion) {
    let engine = Engine::new();
    let entity = engine
        .create_entity(CreateEntityRequest {
            name: "Lookup Test".to_string(),
            description: None,
            tags: None,
        })
        .unwrap();
    let id = entity.id;

    c.bench_function("engine_get_entity", |b| {
        b.iter(|| {
            let _ = engine.get_entity(black_box(id)).unwrap();
        })
    });
}

fn bench_engine_list_entities(c: &mut Criterion) {
    let engine = Engine::new();
    for i in 0..100 {
        engine
            .create_entity(CreateEntityRequest {
                name: format!("Entity {}", i),
                description: None,
                tags: None,
            })
            .unwrap();
    }

    c.bench_function("engine_list_entities_100", |b| {
        b.iter(|| {
            let _ = engine.list_entities(black_box(None)).unwrap();
        })
    });
}

fn bench_engine_update_entity(c: &mut Criterion) {
    let engine = Engine::new();
    let entity = engine
        .create_entity(CreateEntityRequest {
            name: "Update Test".to_string(),
            description: None,
            tags: None,
        })
        .unwrap();
    let id = entity.id;

    c.bench_function("engine_update_entity", |b| {
        let mut counter = 0u64;
        b.iter(|| {
            counter += 1;
            let update = UpdateEntityRequest {
                name: Some(format!("Updated {}", counter)),
                status: Some(Status::Active),
                ..Default::default()
            };
            let _ = engine
                .update_entity(black_box(id), black_box(update))
                .unwrap();
        })
    });
}

criterion_group!(
    routing_benches,
    bench_engine_create_entity,
    bench_engine_get_entity,
    bench_engine_list_entities,
    bench_engine_update_entity,
);
criterion_main!(routing_benches);
