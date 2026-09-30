//! What one body composite costs, on the real install: the naked body and a crowd of distinct
//! tier-2 looks, each read, decoded and blitted in full, as a cache miss does, and what planning
//! the same crowd costs.

use std::time::{Duration, Instant};

use benilla_formats::{load_item_display_catalog, open_chain, CharSections, ItemDisplay};

/// Worn display ids by bodyslot − 2 (shirt, chest, belt, pants, boots, wrist, gloves, tabard):
/// the tier-2 sets at their 1.12 displays, a linen shirt under the first.
const SETS: [[u32; 8]; 9] = [
    [10840, 33983, 33990, 33986, 33989, 33982, 33984, 0],
    [0, 33650, 31110, 31115, 31111, 31127, 33651, 0],
    [0, 33667, 33665, 33672, 34269, 33666, 33668, 0],
    [0, 34081, 34078, 34084, 34083, 34079, 34082, 0],
    [0, 33635, 33633, 33637, 33639, 33634, 33636, 0],
    [0, 34014, 34011, 29857, 34015, 34012, 34013, 0],
    [0, 34038, 34046, 34039, 34044, 34045, 34041, 0],
    [0, 34047, 34053, 34049, 34055, 34052, 34051, 0],
    [0, 30536, 30541, 30540, 30542, 30548, 34016, 0],
];

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort_unstable();
    v[v.len() / 2]
}

#[test]
#[ignore = "instrument: run by hand — cargo test --release -p benilla-formats --test composite_cost -- --ignored --nocapture"]
fn one_composite_costs() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");
    let cs = CharSections::load(&mut chain).expect("CharSections");
    let items = load_item_display_catalog(&mut chain).expect("ItemDisplayInfo");
    let worn = |set: &[u32; 8]| -> [Option<&ItemDisplay>; 8] {
        std::array::from_fn(|i| (set[i] != 0).then(|| items.get(set[i])).flatten())
    };

    // One look, over and over: the body's whole read-decode-blit, the OS file cache warm.
    let naked = (0..20)
        .map(|_| {
            let t = Instant::now();
            let atlas = cs
                .composite_body(&mut chain, 1, 0, 0, 0, 0, 0, 0, [None; 8], None, false)
                .expect("composite")
                .expect("base row");
            std::hint::black_box(atlas);
            t.elapsed()
        })
        .collect::<Vec<_>>();
    let geared = (0..20)
        .map(|_| {
            let t = Instant::now();
            let atlas = cs
                .composite_body(&mut chain, 1, 0, 0, 0, 0, 0, 0, worn(&SETS[0]), None, false)
                .expect("composite")
                .expect("base row");
            std::hint::black_box(atlas);
            t.elapsed()
        })
        .collect::<Vec<_>>();
    println!(
        "COMPOSITE naked human male: median {:.2} ms",
        median(naked).as_secs_f64() * 1e3
    );
    println!(
        "COMPOSITE tier-2 human male: median {:.2} ms",
        median(geared).as_secs_f64() * 1e3
    );

    // A crowd of 40 distinct looks arriving together: eight races, both sexes, the nine sets.
    let races = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let t = Instant::now();
    let mut built = 0;
    for i in 0..40usize {
        let (race, sex) = (races[i % races.len()], (i / races.len() % 2) as u8);
        let set = &SETS[i % SETS.len()];
        if let Ok(Some(atlas)) = cs.composite_body(
            &mut chain,
            race,
            sex,
            0,
            (i % 3) as u8,
            0,
            (i % 4) as u8,
            0,
            worn(set),
            None,
            false,
        ) {
            std::hint::black_box(atlas);
            built += 1;
        }
    }
    let crowd = t.elapsed();
    println!(
        "COMPOSITE crowd: {built} distinct geared looks in {:.1} ms ({:.2} ms each)",
        crowd.as_secs_f64() * 1e3,
        crowd.as_secs_f64() * 1e3 / built.max(1) as f64
    );

    // The same crowd's plans alone: the table lookups a body's first frame does before its
    // composite leaves the thread.
    let t = Instant::now();
    let mut planned = 0;
    for i in 0..40usize {
        let (race, sex) = (races[i % races.len()], (i / races.len() % 2) as u8);
        let set = &SETS[i % SETS.len()];
        if let Some(plan) = cs.composite_plan(
            race,
            sex,
            0,
            (i % 3) as u8,
            0,
            (i % 4) as u8,
            0,
            worn(set),
            None,
            false,
        ) {
            std::hint::black_box(plan);
            planned += 1;
        }
    }
    println!(
        "COMPOSITE crowd plans: {planned} in {:.3} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
}
