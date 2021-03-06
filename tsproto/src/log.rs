use std::fmt::Debug;
use std::net::SocketAddr;

use slog::{debug, error, o, Logger};

use crate::connection::Connection;
use crate::connectionmanager::ConnectionManager;
use crate::handler_data::{
	Data, InCommandObserver, InPacketObserver, InUdpPacketObserver,
	OutPacketObserver, OutUdpPacketObserver,
};
use crate::packets::{
	Direction, InCommand, InPacket, InUdpPacket, OutPacket, PacketType,
};

fn prepare_logger(logger: &Logger, is_client: bool, incoming: bool) -> Logger {
	let in_s = if incoming {
		if !cfg!(windows) {
			"\x1b[1;32mIN\x1b[0m"
		} else {
			"IN"
		}
	} else if !cfg!(windows) {
		"\x1b[1;31mOUT\x1b[0m"
	} else {
		"OUT"
	};
	let to_s = if is_client { "S" } else { "C" };
	logger.new(o!("to" => to_s, "dir" => in_s))
}

pub fn log_udp_packet<P: Debug>(
	logger: &Logger,
	addr: SocketAddr,
	is_client: bool,
	incoming: bool,
	packet: &P,
)
{
	let logger =
		prepare_logger(&logger.new(o!("addr" => addr)), is_client, incoming);
	debug!(logger, "UdpPacket"; "content" => ?packet);
}

pub fn log_packet<P: Debug>(
	logger: &Logger,
	is_client: bool,
	incoming: bool,
	packet: &P,
)
{
	// packet.header.c_id is not set for newly created packets so we cannot
	// detect if a packet is incoming or not.
	let logger = prepare_logger(logger, is_client, incoming);
	debug!(logger, "Packet"; "content" => ?packet);
}

pub fn log_command(
	logger: &Logger,
	is_client: bool,
	incoming: bool,
	p_type: PacketType,
	cmd: &str,
)
{
	// packet.header.c_id is not set for newly created packets so we cannot
	// detect if a packet is incoming or not.
	let logger = prepare_logger(logger, is_client, incoming);
	if p_type == PacketType::Command {
		debug!(logger, "Command"; "content" => cmd);
	} else {
		debug!(logger, "CommandLow"; "content" => cmd);
	}
}

#[derive(Clone, Debug)]
struct UdpPacketLogger {
	logger: Logger,
	is_client: bool,
}
impl InUdpPacketObserver for UdpPacketLogger {
	fn observe(&self, addr: SocketAddr, udp_packet: &InPacket) {
		let udp_packet = InUdpPacket::new(udp_packet);
		log_udp_packet(&self.logger, addr, self.is_client, true, &udp_packet);
	}
}

impl OutUdpPacketObserver for UdpPacketLogger {
	fn observe(&self, addr: SocketAddr, udp_packet: &[u8]) {
		match InPacket::try_new(
			udp_packet.into(),
			if self.is_client {
				Direction::C2S
			} else {
				Direction::S2C
			},
		) {
			Ok(packet) => log_udp_packet(
				&self.logger,
				addr,
				self.is_client,
				false,
				&packet,
			),
			Err(e) => {
				error!(self.logger, "Cannot parse incoming udp packet"; "error" => ?e)
			}
		}
	}
}

#[derive(Clone, Debug)]
struct PacketLogger {
	is_client: bool,
}
impl<T: Send> InPacketObserver<T> for PacketLogger {
	fn observe(&self, con: &mut (T, Connection), packet: &InPacket) {
		log_packet(&con.1.logger, self.is_client, true, packet);
	}
}

impl<T: Send> OutPacketObserver<T> for PacketLogger {
	fn observe(&self, con: &mut (T, Connection), packet: &mut OutPacket) {
		log_packet(&con.1.logger, self.is_client, false, packet);
	}
}

#[derive(Clone, Debug)]
struct CommandLogger {
	is_client: bool,
}
impl<T: Send> InCommandObserver<T> for CommandLogger {
	fn observe(&self, con: &mut (T, Connection), cmd: &InCommand) {
		let cmd_s = ::std::str::from_utf8(cmd.content()).unwrap();
		log_command(
			&con.1.logger,
			self.is_client,
			true,
			cmd.packet_type(),
			cmd_s,
		);
	}
}

impl<T: Send> OutPacketObserver<T> for CommandLogger {
	fn observe(&self, con: &mut (T, Connection), packet: &mut OutPacket) {
		let p_type = packet.header().packet_type();
		if p_type == PacketType::Command || p_type == PacketType::CommandLow {
			let cmd_s = ::std::str::from_utf8(packet.content()).unwrap();
			log_command(&con.1.logger, self.is_client, false, p_type, cmd_s);
		}
	}
}

pub fn add_udp_packet_logger<CM: ConnectionManager + 'static>(
	data: &mut Data<CM>,
) {
	data.add_in_udp_packet_observer(
		"log".into(),
		Box::new(UdpPacketLogger {
			logger: data.logger.clone(),
			is_client: data.is_client,
		}),
	);
	data.add_out_udp_packet_observer(
		"log".into(),
		Box::new(UdpPacketLogger {
			logger: data.logger.clone(),
			is_client: data.is_client,
		}),
	);
}

pub fn add_packet_logger<CM: ConnectionManager + 'static>(data: &mut Data<CM>) {
	data.add_in_packet_observer(
		"log".into(),
		Box::new(PacketLogger {
			is_client: data.is_client,
		}),
	);
	data.add_out_packet_observer(
		"log".into(),
		Box::new(PacketLogger {
			is_client: data.is_client,
		}),
	);
}

pub fn add_command_logger<CM: ConnectionManager + 'static>(
	data: &mut Data<CM>,
) {
	data.add_in_command_observer(
		"cmdlog".into(),
		Box::new(CommandLogger {
			is_client: data.is_client,
		}),
	);
	data.add_out_packet_observer(
		"cmdlog".into(),
		Box::new(CommandLogger {
			is_client: data.is_client,
		}),
	);
}
