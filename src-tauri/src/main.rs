fn main() {
    #[cfg(debug_assertions)]
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    #[cfg(not(debug_assertions))]
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    xxmi_nrmm_lib::run();
}
