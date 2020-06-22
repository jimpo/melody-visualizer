use jack::{AudioIn, NotificationHandler};

use crate::error::Error;
//
// pub struct AudioSourceController {
// 	client: jack::AsyncClient<Self, Self>,
// }
//
// impl AudioSourceController {
// 	pub fn new() -> Result<Self, Error> {
// 		let (client, status) = jack::Client::new(TITLE, jack::ClientOptions::NO_START_SERVER)
// 			.map_err(Error::Jack)?;
// 		if !status.is_empty() {
// 			return Err(Error::JackStatus(status));
// 		}
//
// 		let port = client.register_port("input", AudioIn)
// 			.map_err(Error::Jack)?;
//
// 		Ok(())
// 	}
//
// 	pub fn client(&self) -> &jack::Client {
// 	    self.client.client()
// 	}
// }

// impl NotificationHandler for AudioSourceController {
//
// }
