//! 确定性合成母版，用于演示分块 API 与本地格式互操作检查。
use macin_atmos::*;
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let output = args
        .get(1)
        .ok_or("usage: synthetic_master OUTPUT [standard|logic|home|3dof] [48000|96000]")?;
    let profile = match args.get(2).map(String::as_str).unwrap_or("standard") {
        "standard" => Profile::Standard,
        "logic" => Profile::Logic,
        "home" => Profile::DamfHome,
        "3dof" => Profile::DamfThreeDof,
        _ => return Err("unknown profile".into()),
    };
    let rate: u32 = args.get(3).map(String::as_str).unwrap_or("48000").parse()?;
    let mut spec = MasterSpec {
        sample_rate: rate,
        profile,
        objects: vec![
            ObjectSpec::new(TrackId(901), "Moving object"),
            ObjectSpec::new(TrackId(902), "Elevated object"),
        ],
        ..Default::default()
    };
    spec.objects[0].initial.position = [-0.8, 0.5, 0.0];
    spec.objects[1].initial.position = [0.8, -0.4, 0.5];
    let mut writer = MasterWriter::create(output, spec)?;
    writer.schedule_updates(&[ObjectUpdate {
        track: TrackId(901),
        sample: u64::from(rate / 2),
        ramp_samples: Some(u64::from(rate)),
        patch: ObjectPatch {
            position: Some([0.8, 0.5, 0.]),
            ..Default::default()
        },
    }])?;
    let total = rate as usize * 2;
    for start in (0..total).step_by(512) {
        let n = 512.min(total - start);
        let mut pcm = vec![0.; n * 12];
        for (offset, frame) in pcm.as_chunks_mut::<12>().0.iter_mut().enumerate() {
            let t = (start + offset) as f64 / f64::from(rate);
            for (channel, hz, level) in [
                (0, 220., 0.03),
                (3, 55., 0.02),
                (10, 440., 0.1),
                (11, 660., 0.08),
            ] {
                frame[channel] = (level * libm::sin(std::f64::consts::TAU * hz * t)) as f32;
            }
        }
        writer.write_block(PcmBlock::F32Interleaved(&pcm), &[])?;
    }
    let report = writer.finish()?;
    println!(
        "{} Hz, {} tracks, {} frames, {} PCM bytes, {} mapping losses",
        report.sample_rate,
        report.channels,
        report.frames,
        report.pcm_bytes,
        report.diagnostics.len()
    );
    Ok(())
}
