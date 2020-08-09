pub trait Configurable {
	type Config;

	fn new(config: Self::Config) -> Self;
	fn set_config(&mut self, config: Self::Config);
}