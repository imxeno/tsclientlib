use std::borrow::Cow;
use std::collections::HashMap;
use std::io::prelude::*;
use std::mem;
use std::str;
use std::str::FromStr;

use nom::types::CompleteStr;
use nom::{alphanumeric, alt, call, do_parse, eof, error_position, is_not, many0,
	many1, map, multispace, named, preceded, opt, tag, tuple, tuple_parser};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
	pub command: String,
	pub static_args: Vec<(String, String)>,
	pub list_args: Vec<Vec<(String, String)>>,
}

named!(command_arg2(CompleteStr) -> (&str, Cow<str>), do_parse!(many0!(multispace) >>
	name: is_not!("\u{b}\u{c}\\\t\r\n| /=") >> // Argument name
	value: map!(opt!( // Argument value
		preceded!(tag!("="),
			do_parse!(
				// Try to parse the value without escaped characters
				prefix: opt!(is_not!("\u{b}\u{c}\\\t\r\n| /")) >>
				rest: many0!(alt!(
					map!(tag!("\\v"), |_| "\x0b") | // Vertical tab
					map!(tag!("\\f"), |_| "\x0c") | // Form feed
					map!(tag!("\\\\"), |_| "\\") |
					map!(tag!("\\t"), |_| "\t") |
					map!(tag!("\\r"), |_| "\r") |
					map!(tag!("\\n"), |_| "\n") |
					map!(tag!("\\p"), |_| "|") |
					map!(tag!("\\s"), |_| " ") |
					map!(tag!("\\/"), |_| "/") |
					map!(is_not!("\u{b}\u{c}\\\t\r\n| /"), |s| *s)
				)) >> (if rest.is_empty() { Cow::Borrowed(prefix.map(|p| *p).unwrap_or("")) }
					else { Cow::Owned(format!("{}{}", prefix.map(|p| *p).unwrap_or(""), rest.concat())) })
			)
		)), |o| o.unwrap_or(Cow::Borrowed("")))
	>> (*name, value)
));

named!(inner_parse_command(CompleteStr) -> CommandData, do_parse!(
	name: alt!(do_parse!(res: alphanumeric >> multispace >> (res)) | tag!("")) >> // Command
	static_args: many0!(command_arg2) >>
	list_args: many0!(do_parse!(many0!(multispace) >>
		tag!("|") >>
		args: many1!(command_arg2) >>
		(args)
	)) >>
	many0!(multispace) >>
	eof!() >>
	(CommandData {
		name: *name,
		static_args,
		list_args,
	})
));

named!(command_arg(CompleteStr) -> (String, String), do_parse!(many0!(multispace) >>
	name: many1!(map!(is_not!("\u{b}\u{c}\\\t\r\n| /="), |s| *s)) >> // Argument name
	value: map!(opt!( // Argument value
		preceded!(tag!("="),
			many0!(alt!(
				map!(tag!("\\v"), |_| "\x0b") | // Vertical tab
				map!(tag!("\\f"), |_| "\x0c") | // Form feed
				map!(tag!("\\\\"), |_| "\\") |
				map!(tag!("\\t"), |_| "\t") |
				map!(tag!("\\r"), |_| "\r") |
				map!(tag!("\\n"), |_| "\n") |
				map!(tag!("\\p"), |_| "|") |
				map!(tag!("\\s"), |_| " ") |
				map!(tag!("\\/"), |_| "/") |
				map!(is_not!("\u{b}\u{c}\\\t\r\n| /"), |s| *s)
			))
		)), |o| o.unwrap_or_default())
	>> (name.concat(), value.concat())
));

named!(parse_command(CompleteStr) -> Command, do_parse!(
	command: alt!(do_parse!(res: alphanumeric >> multispace >> (res)) | tag!("")) >> // Command
	static_args: many0!(command_arg) >>
	list_args: many0!(do_parse!(many0!(multispace) >>
		tag!("|") >>
		args: many1!(command_arg) >>
		(args)
	)) >>
	many0!(multispace) >>
	eof!() >>
	(Command {
		command: command.to_string(),
		static_args,
		list_args,
	})
));

#[derive(Debug, Clone)]
pub struct CommandData<'a> {
	/// The name is empty for serverquery commands
	pub name: &'a str,
	pub static_args: Vec<(&'a str, Cow<'a, str>)>,
	pub list_args: Vec<Vec<(&'a str, Cow<'a, str>)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalCommand<'a>(pub HashMap<&'a str, &'a str>);
impl<'a> CanonicalCommand<'a> {
	pub fn has(&self, arg: &str) -> bool { self.0.contains_key(arg) }
	pub fn get(&self, arg: &str) -> Option<&str> { self.0.get(arg).map(|s| *s) }

	pub fn get_parse<F: FromStr>(&self, arg: &str) -> std::result::Result<F, Option<<F as FromStr>::Err>> {
		if let Some(s) = self.0.get(arg) {
			s.parse::<F>().map_err(Some)
		} else {
			Err(None)
		}
	}
}

pub fn parse_command2(s: &str) -> Result<CommandData> {
	match inner_parse_command(CompleteStr(s)) {
		Ok((rest, mut cmd)) => {
			// Error if rest contains something
			if !rest.is_empty() {
				return Err(crate::Error::ParseCommand(format!(
					"Command was not parsed completely {:?}",
					rest
				)));
			}

			// Some of the static args are variable so move the to the right
			// category.
			if !cmd.list_args.is_empty() {
				let mut la = Vec::new();
				for &(ref arg, _) in &cmd.list_args[0] {
					if let Some(i) =
						cmd.static_args.iter().position(|&(ref k, _)| k == arg)
					{
						la.push(cmd.static_args.remove(i));
					} else {
						// Not a valid command list, but ignore it
					}
				}
				cmd.list_args.insert(0, la);
			}
			Ok(cmd)
		}
		Err(e) => Err(crate::Error::ParseCommand(format!("{:?}", e))),
	}
}

pub struct CommandDataIterator<'a> {
	cmd: &'a CommandData<'a>,
	pub statics: HashMap<&'a str, &'a str>,
	i: usize,
}

impl<'a> Iterator for CommandDataIterator<'a> {
	type Item = CanonicalCommand<'a>;
	fn next(&mut self) -> Option<Self::Item> {
		let i = self.i;
		self.i += 1;
		if self.cmd.list_args.is_empty() {
			if i == 0 {
				Some(CanonicalCommand(mem::replace(
					&mut self.statics,
					HashMap::new(),
				)))
			} else {
				None
			}
		} else if i < self.cmd.list_args.len() {
			let l = &self.cmd.list_args[i];
			let mut v = self.statics.clone();
			v.extend(l.iter().map(|(k, v)| (*k, v.as_ref())));
			Some(CanonicalCommand(v))
		} else {
			None
		}
	}
}

impl<'a> CommandData<'a> {
	pub fn static_arg(&self, k: &str) -> Option<&str> {
		self.static_args.iter().find_map(|(k2, v)| if *k2 == k
			{ Some(v.as_ref()) } else { None })
	}

	pub fn iter(&self) -> CommandDataIterator {
		let statics = self
			.static_args
			.iter()
			.map(|(a, b)| (*a, b.as_ref()))
			.collect();
		CommandDataIterator {
			cmd: self,
			statics,
			i: 0,
		}
	}
}

impl Command {
	pub fn new<T: Into<String>>(command: T) -> Command {
		Command {
			command: command.into(),
			static_args: Vec::new(),
			list_args: Vec::new(),
		}
	}

	pub fn push<K: Into<String>, V: Into<String>>(&mut self, key: K, val: V) {
		self.static_args.push((key.into(), val.into()));
	}

	/// Replace an argument if it exists.
	pub fn replace<K: Into<String>, V: Into<String>>(
		&mut self,
		key: K,
		val: V,
	)
	{
		let key = key.into();
		for &mut (ref mut k, ref mut v) in &mut self.static_args {
			if key == *k {
				*v = val.into();
				break;
			}
		}
	}

	/// Remove an argument if it exists.
	pub fn remove<K: Into<String>>(&mut self, key: K) {
		let key = key.into();
		self.static_args.retain(|&(ref k, _)| *k != key);
	}

	/// Check, if each list argument is contained in each list.
	pub fn is_valid(&self) -> bool {
		if !self.list_args.is_empty() {
			let first = &self.list_args[0];
			for l in &self.list_args[1..] {
				if l.len() != first.len() {
					return false;
				}
				for &(ref arg, _) in first {
					if l.iter().any(|&(ref a, _)| a == arg) {
						return false;
					}
				}
			}
		}
		true
	}

	pub fn read<T>(_: T, r: &mut Read) -> Result<Command> {
		let mut buf = String::new();
		r.read_to_string(&mut buf)?;
		match parse_command(CompleteStr(&buf)) {
			Ok((rest, mut cmd)) => {
				// Error if rest contains something
				if !rest.is_empty() {
					return Err(crate::Error::ParseCommand(format!(
						"Command was not parsed completely {:?}",
						rest
					)));
				}

				// Some of the static args are variable so move the to the right
				// category.
				if !cmd.list_args.is_empty() {
					let mut la = Vec::new();
					for &(ref arg, _) in &cmd.list_args[0] {
						if let Some(i) = cmd
							.static_args
							.iter()
							.position(|&(ref k, _)| k == arg)
						{
							la.push(cmd.static_args.remove(i));
						} else {
							// Not a valid command list, but ignore it
						}
					}
					cmd.list_args.insert(0, la);
				}
				Ok(cmd)
			}
			Err(e) => Err(crate::Error::ParseCommand(format!("{:?}", e))),
		}
	}

	fn write_escaped(w: &mut Write, s: &str) -> Result<()> {
		for c in s.chars() {
			match c {
				'\u{b}' => write!(w, "\\v"),
				'\u{c}' => write!(w, "\\f"),
				'\\' => write!(w, "\\\\"),
				'\t' => write!(w, "\\t"),
				'\r' => write!(w, "\\r"),
				'\n' => writeln!(w),
				'|' => write!(w, "\\p"),
				' ' => write!(w, "\\s"),
				'/' => write!(w, "\\/"),
				c => write!(w, "{}", c),
			}?;
		}
		Ok(())
	}

	fn write_key_val(w: &mut Write, k: &str, v: &str) -> Result<()> {
		if v.is_empty() && k != "return_code" {
			write!(w, "{}", k)?;
		} else {
			write!(w, "{}=", k)?;
			Self::write_escaped(w, v)?;
		}
		Ok(())
	}

	pub fn write(&self, w: &mut Write) -> Result<()> {
		w.write_all(self.command.as_bytes())?;
		for &(ref k, ref v) in &self.static_args {
			write!(w, " ")?;
			Self::write_key_val(w, k, v)?;
		}
		for (i, args) in self.list_args.iter().enumerate() {
			if i != 0 {
				write!(w, "|")?;
			}
			for (j, &(ref k, ref v)) in args.iter().enumerate() {
				if j != 0 || i == 0 {
					write!(w, " ")?;
				}
				Self::write_key_val(w, k, v)?;
			}
		}
		Ok(())
	}

	pub fn has_arg(&self, arg: &str) -> bool {
		if self.static_args.iter().any(|&(ref a, _)| a == arg) {
			true
		} else if !self.list_args.is_empty() {
			self.list_args[0].iter().any(|&(ref a, _)| a == arg)
		} else {
			false
		}
	}

	pub fn get_static_arg<K: AsRef<str>>(&self, key: K) -> Option<&str> {
		let key = key.as_ref();
		self.static_args
			.iter()
			.filter_map(
				|&(ref k, ref v)| {
					if k == key {
						Some(v.as_str())
					} else {
						None
					}
				},
			)
			.next()
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;
	use std::io::Cursor;
	use std::iter::FromIterator;

	use super::parse_command2;
	use crate::commands::Command;

	#[test]
	fn parse() {
		let s = b"cmd a=1 b=2 c=3";
		let mut cmd = Command::new("cmd");
		cmd.push("a", "1");
		cmd.push("b", "2");
		cmd.push("c", "3");

		// Read
		let cmd_r = Command::read((), &mut Cursor::new(s)).unwrap();
		assert_eq!(cmd, cmd_r);
		// Write
		let mut s_r = Vec::new();
		cmd.write(&mut s_r).unwrap();
		assert_eq!(&s[..], s_r.as_slice());
	}

	#[test]
	fn escape() {
		let s = b"cmd a=\\s\\\\ b=\\p c=abc\\tdef";
		let mut cmd = Command::new("cmd");
		cmd.push("a", " \\");
		cmd.push("b", "|");
		cmd.push("c", "abc\tdef");

		// Read
		let cmd_r = Command::read((), &mut Cursor::new(s)).unwrap();
		assert_eq!(cmd, cmd_r);
		// Write
		let mut s_r = Vec::new();
		cmd.write(&mut s_r).unwrap();
		assert_eq!(&s[..], s_r.as_slice());
	}

	#[test]
	fn array() {
		let s = b"cmd a=1 c=3 b=2|b=4|b=5";
		let mut cmd = Command::new("cmd");
		cmd.push("a", "1");
		cmd.push("c", "3");

		cmd.list_args.push(vec![("b".into(), "2".into())]);
		cmd.list_args.push(vec![("b".into(), "4".into())]);
		cmd.list_args.push(vec![("b".into(), "5".into())]);

		// Read
		let cmd_r = Command::read((), &mut Cursor::new(s)).unwrap();
		assert_eq!(cmd, cmd_r);
		// Write
		let mut s_r = Vec::new();
		cmd.write(&mut s_r).unwrap();
		assert_eq!(&s[..], s_r.as_slice());
	}

	#[test]
	fn optional_arg() {
		let s = b"cmd a";
		Command::read((), &mut Cursor::new(s.as_ref())).unwrap();
		let s = b"cmd a b=1";
		Command::read((), &mut Cursor::new(s.as_ref())).unwrap();
		let s = b"cmd a=";
		Command::read((), &mut Cursor::new(s.as_ref())).unwrap();
		let s = b"cmd a= b=1";
		Command::read((), &mut Cursor::new(s.as_ref())).unwrap();
	}

	#[test]
	fn initivexpand2() {
		let s = "initivexpand2 l=AQCVXTlKF+UQc0yga99dOQ9FJCwLaJqtDb1G7xYPMvHFMwIKVfKADF6zAAcAAAAgQW5vbnltb3VzAAAKQo71lhtEMbqAmtuMLlY8Snr0k2Wmymv4hnHNU6tjQCALKHewCykgcA== beta=\\/8kL8lcAYyMJovVOP6MIUC1oZASyuL\\/Y\\/qjVG06R4byuucl9oPAvR7eqZI7z8jGm9jkGmtJ6 omega=MEsDAgcAAgEgAiBxu2eCLQf8zLnuJJ6FtbVjfaOa1210xFgedoXuGzDbTgIgcGk35eqFavKxS4dROi5uKNSNsmzIL4+fyh5Z\\/+FWGxU= ot=1 proof=MEUCIQDRCP4J9e+8IxMJfCLWWI1oIbNPGcChl+3Jr2vIuyDxzAIgOrzRAFPOuJZF4CBw\\/xgbzEsgKMtEtgNobF6WXVNhfUw= tvd time=1544221457";
		Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		parse_command2(s.as_ref()).unwrap();
	}

	#[test]
	fn clientinitiv() {
		let s = "clientinitiv alpha=41Te9Ar7hMPx+A== omega=MEwDAgcAAgEgAiEAq2iCMfcijKDZ5tn2tuZcH+\\/GF+dmdxlXjDSFXLPGadACIHzUnbsPQ0FDt34Su4UXF46VFI0+4wjMDNszdoDYocu0 ip";
		Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		parse_command2(s.as_ref()).unwrap();
	}

	#[test]
	fn initserver() {
		// Well, that's more corrupted packet, but the parser should be robust
		let s =
			"initserver virtualserver_name=Server\\sder\\sVerplanten \
			 virtualserver_welcomemessage=This\\sis\\sSplamys\\sWorld \
			 virtualserver_platform=Linux \
			 virtualserver_version=3.0.13.8\\s[Build:\\s1500452811] \
			 virtualserver_maxclients=32 virtualserver_created=0 \
			 virtualserver_nodec_encryption_mode=1 \
			 virtualserver_hostmessage=Lé\\sServer\\sde\\sSplamy \
			 virtualserver_name=Server_mode=0 virtualserver_default_server \
			 group=8 virtualserver_default_channel_group=8 \
			 virtualserver_hostbanner_url virtualserver_hostmessagegfx_url \
			 virtualserver_hostmessagegfx_interval=2000 \
			 virtualserver_priority_speaker_dimm_modificat";
		Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		parse_command2(s.as_ref()).unwrap();
	}

	#[test]
	fn channellist() {
		let s =
			"channellist cid=2 cpid=0 channel_name=Trusted\\sChannel \
			 channel_topic channel_codec=0 channel_codec_quality=0 \
			 channel_maxclients=0 channel_maxfamilyclients=-1 channel_order=1 \
			 channel_flag_permanent=1 channel_flag_semi_permanent=0 \
			 channel_flag_default=0 channel_flag_password=0 \
			 channel_codec_latency_factor=1 channel_codec_is_unencrypted=1 \
			 channel_delete_delay=0 channel_flag_maxclients_unlimited=0 \
			 channel_flag_maxfamilyclients_unlimited=0 \
			 channel_flag_maxfamilyclients_inherited=1 \
			 channel_needed_talk_power=0 channel_forced_silence=0 \
			 channel_name_phonetic channel_icon_id=0 \
			 channel_flag_private=0|cid=4 cpid=2 \
			 channel_name=Ding\\s•\\s1\\s\\p\\sSplamy´s\\sBett channel_topic \
			 channel_codec=4 channel_codec_quality=7 channel_maxclients=-1 \
			 channel_maxfamilyclients=-1 channel_order=0 \
			 channel_flag_permanent=1 channel_flag_semi_permanent=0 \
			 channel_flag_default=0 channel_flag_password=0 \
			 channel_codec_latency_factor=1 channel_codec_is_unencrypted=1 \
			 channel_delete_delay=0 channel_flag_maxclients_unlimited=1 \
			 channel_flag_maxfamilyclients_unlimited=0 \
			 channel_flag_maxfamilyclients_inherited=1 \
			 channel_needed_talk_power=0 channel_forced_silence=0 \
			 channel_name_phonetic=Neo\\sSeebi\\sEvangelion channel_icon_id=0 \
			 channel_flag_private=0"; //|cid=6 cpid=2 channel_name=Ding\\s\xe2\x80\xa2\\s2\\s\\p\\sThe\\sBook\\sof\\sHeavy\\sMetal channel_topic channel_codec=2 channel_codec_quality=7 channel_maxclients=-1 channel_maxfamilyclients=-1 channel_order=4 channel_flag_permanent=1 channel_flag_semi_permanent=0 channel_flag_default=0 channel_flag_password=0 channel_codec_latency_factor=1 channel_codec_is_unencrypted=1 channel_delete_delay=0 channel_flag_maxclients_unlimited=1 channel_flag_maxfamilyclients_unlimited=0 channel_flag_maxfamilyclients_inherited=1 channel_needed_talk_power=0 channel_forced_silence=0 channel_name_phonetic=Not\\senought\\sChannels channel_icon_id=0 channel_flag_private=0|cid=30 cpid=2 channel_name=Ding\\s\xe2\x80\xa2\\s3\\s\\p\\sSenpai\\sGef\xc3\xa4hrlich channel_topic channel_codec=2 channel_codec_quality=7 channel_maxclients=-1 channel_maxfamilyclients=-1 channel_order=6 channel_flag_permanent=1 channel_flag_semi_permanent=0 channel_flag_default=0 channel_flag_password=0 channel_codec_latency_factor=1 channel_codec_is_unencrypted=1 channel_delete_delay=0 channel_flag_maxclients_unlimited=1 channel_flag_maxfamilyclients_unlimited=0 channel_flag_maxfamilyclients_inherited=1 channel_needed_talk_power=0 channel_forced_silence=0 channel_name_phonetic=The\\strashcan\\shas\\sthe\\strash channel_icon_id=0 channel_flag_private=0";
		Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		parse_command2(s.as_ref()).unwrap();
	}

	#[test]
	fn subscribe() {
		let s = "notifychannelsubscribed cid=2|cid=4 es=3867|cid=5 \
		         es=18694|cid=6 es=18694|cid=7 es=18694|cid=11 \
		         es=18694|cid=13 es=18694|cid=14 es=18694|cid=16 \
		         es=18694|cid=22 es=18694|cid=23 es=18694|cid=24 \
		         es=18694|cid=25 es=18694|cid=30 es=18694|cid=163 es=18694";
		Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		parse_command2(s.as_ref()).unwrap();
	}

	#[test]
	fn permissionlist() {
		let s = "notifypermissionlist group_id_end=0|group_id_end=7|group_id_end=13|group_id_end=18|group_id_end=21|group_id_end=21|group_id_end=33|group_id_end=47|group_id_end=77|group_id_end=82|group_id_end=83|group_id_end=106|group_id_end=126|group_id_end=132|group_id_end=143|group_id_end=151|group_id_end=160|group_id_end=162|group_id_end=170|group_id_end=172|group_id_end=190|group_id_end=197|group_id_end=215|group_id_end=227|group_id_end=232|group_id_end=248|permname=b_serverinstance_help_view permdesc=Retrieve\\sinformation\\sabout\\sServerQuery\\scommands|permname=b_serverinstance_version_view permdesc=Retrieve\\sglobal\\sserver\\sversion\\s(including\\splatform\\sand\\sbuild\\snumber)|permname=b_serverinstance_info_view permdesc=Retrieve\\sglobal\\sserver\\sinformation|permname=b_serverinstance_virtualserver_list permdesc=List\\svirtual\\sservers\\sstored\\sin\\sthe\\sdatabase";
		Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		parse_command2(s.as_ref()).unwrap();
	}

	#[test]
	fn serverquery_command() {
		let s = "cmd=1 cid=2";
		let cmd = Command::read((), &mut Cursor::new(s.as_bytes())).unwrap();
		let in_cmd = parse_command2(s.as_ref()).unwrap();
		assert_eq!(cmd.command, "");
		assert_eq!(in_cmd.name, "");
	}
}
