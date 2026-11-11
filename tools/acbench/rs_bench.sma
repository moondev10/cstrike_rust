// cstrike-game AC bench probe (AMX Mod X). Answers "does the client behave like the original"
// for the checks server-side anti-cheats rely on:
//   Q phase: query_client_cvar for every name in configs/rs_bench_cvars.ini, logged as
//            "Q|<name>|<reply>" (the reply is "Bad CVAR request" for an unknown cvar).
//   S phase: for every "<name> <value>" line in configs/rs_bench_stuff.ini: query, stufftext
//            "<name> <value>" twice, query again after 0.3 s, restore the first reply; logged as
//            "S|<name>|<before>|<after>".
//   X phase (docs/re/audit-ac-checks.md §6): stufftext semantics (P1), local commands that must
//            not reach the server (P2), query edge cases (P4), the MOTD browser (P6), director
//            stufftext (P9) and byte fidelity (P10), logged as "X|<probe>|<value>".
//   Throughout: every client command the game DLL sees ("C|<argv0>|<args>", P2/P8) and the
//   usercmd accounting per 10 s ("P|<ratio>|<cmds>|<min lerp>|<max lerp>|<bad moves>", P5).
// Ends with "DONE" (Q and S) and "XDONE". Output: logs/rs_bench.log.

#include <amxmodx>
#include <amxmisc>
#include <fakemeta>

#define MAX_NAMES 600
#define BATCH 16

new g_names[MAX_NAMES][48];
new g_count;
new g_stuff_names[64][48];
new g_stuff_values[64][64];
new g_stuff_count;

new g_next[MAX_PLAYERS + 1];
new g_stuff_next[MAX_PLAYERS + 1];
new g_before[MAX_PLAYERS + 1][128];

public plugin_init()
{
	register_plugin("cstrike-game bench", "1.0", "cstrike-game");
	new dir[128], path[160], line[160];
	get_configsdir(dir, charsmax(dir));

	formatex(path, charsmax(path), "%s/rs_bench_cvars.ini", dir);
	new f = fopen(path, "rt");
	if (f)
	{
		while (!feof(f) && g_count < MAX_NAMES)
		{
			fgets(f, line, charsmax(line));
			trim(line);
			if (line[0] && line[0] != ';')
				copy(g_names[g_count++], charsmax(g_names[]), line);
		}
		fclose(f);
	}

	formatex(path, charsmax(path), "%s/rs_bench_stuff.ini", dir);
	f = fopen(path, "rt");
	if (f)
	{
		new name[48], value[64];
		while (!feof(f) && g_stuff_count < sizeof g_stuff_names)
		{
			fgets(f, line, charsmax(line));
			trim(line);
			if (!line[0] || line[0] == ';')
				continue;
			parse(line, name, charsmax(name), value, charsmax(value));
			copy(g_stuff_names[g_stuff_count], charsmax(g_stuff_names[]), name);
			copy(g_stuff_values[g_stuff_count], charsmax(g_stuff_values[]), value);
			g_stuff_count++;
		}
		fclose(f);
	}
	log_to_file("rs_bench.log", "LOADED|%d|%d", g_count, g_stuff_count);
	register_forward(FM_CmdStart, "fw_cmd_start");
	register_message(get_user_msgid("ReqState"), "on_req_state");
}

// P8: the game DLL asks for the voice state (CVoiceGameMgr::UpdateMasks, while it has no
// VModEnable from the client); the client answers with VModEnable and vban.
public on_req_state(msg_id, msg_dest, msg_entity)
{
	log_to_file("rs_bench.log", "R|%d", msg_entity);
}

// ------------------------------------------------------------------ P2 / P8: client commands

public client_command(id)
{
	new cmd[64], args[192];
	read_argv(0, cmd, charsmax(cmd));
	read_args(args, charsmax(args));
	log_to_file("rs_bench.log", "C|%s|%s", cmd, args);
	return PLUGIN_CONTINUE;
}

// ------------------------------------------------------------------ P5: usercmd accounting

new Float:g_win_start[MAX_PLAYERS + 1];
new g_win_msec[MAX_PLAYERS + 1];
new g_win_cmds[MAX_PLAYERS + 1];
new g_win_lerp_min[MAX_PLAYERS + 1];
new g_win_lerp_max[MAX_PLAYERS + 1];
new g_win_bad[MAX_PLAYERS + 1];

public fw_cmd_start(id, uc, seed)
{
	if (id < 1 || id > MAX_PLAYERS || !is_user_connected(id))
		return FMRES_IGNORED;
	new Float:now = get_gametime();
	if (g_win_start[id] == 0.0)
	{
		g_win_start[id] = now;
		g_win_lerp_min[id] = 1000;
	}
	new msec = get_uc(uc, UC_Msec);
	new lerp = get_uc(uc, UC_LerpMsec);
	new buttons = get_uc(uc, UC_Buttons);
	new Float:fwd, Float:side;
	get_uc(uc, UC_ForwardMove, fwd);
	get_uc(uc, UC_SideMove, side);
	g_win_msec[id] += msec;
	g_win_cmds[id]++;
	if (lerp < g_win_lerp_min[id])
		g_win_lerp_min[id] = lerp;
	if (lerp > g_win_lerp_max[id])
		g_win_lerp_max[id] = lerp;
	if ((side != 0.0 && !(buttons & (IN_MOVELEFT | IN_MOVERIGHT))) || (fwd != 0.0 && !(buttons & (IN_FORWARD | IN_BACK))))
		g_win_bad[id]++;
	if (now - g_win_start[id] >= 10.0)
	{
		new Float:ratio = float(g_win_msec[id]) / 1000.0 / (now - g_win_start[id]);
		log_to_file("rs_bench.log", "P|%.3f|%d|%d|%d|%d", ratio, g_win_cmds[id], g_win_lerp_min[id], g_win_lerp_max[id], g_win_bad[id]);
		g_win_start[id] = now;
		g_win_msec[id] = 0;
		g_win_cmds[id] = 0;
		g_win_lerp_min[id] = 1000;
		g_win_lerp_max[id] = 0;
		g_win_bad[id] = 0;
	}
	return FMRES_IGNORED;
}

// P4: a query while the client is still signing on (anti_bota's `team`).
public client_connect(id)
{
	set_task(1.0, "x_signon_query", id + 1000);
}

public x_signon_query(task)
{
	new id = task - 1000;
	if (is_user_connecting(id) || is_user_connected(id))
		query_client_cvar(id, "team", "on_signon");
}

public on_signon(id, const cvar[], const value[])
{
	log_to_file("rs_bench.log", "X|signon_%s|%s", cvar, value);
}

public client_putinserver(id)
{
	if (is_user_bot(id) || is_user_hltv(id))
		return;
	g_next[id] = 0;
	g_stuff_next[id] = 0;
	set_task(3.0, "query_batch", id);
}

public client_disconnected(id)
{
	remove_task(id);
}

public query_batch(id)
{
	if (!is_user_connected(id))
		return;
	for (new i = 0; i < BATCH && g_next[id] < g_count; i++)
	{
		query_client_cvar(id, g_names[g_next[id]], "on_query");
		g_next[id]++;
	}
	if (g_next[id] < g_count)
		set_task(0.2, "query_batch", id);
	else
		set_task(2.0, "stuff_next", id);
}

public on_query(id, const cvar[], const value[])
{
	log_to_file("rs_bench.log", "Q|%s|%s", cvar, value);
}

public stuff_next(id)
{
	if (!is_user_connected(id))
		return;
	if (g_stuff_next[id] >= g_stuff_count)
	{
		log_to_file("rs_bench.log", "DONE");
		set_task(1.0, "x_p1", id);
		return;
	}
	query_client_cvar(id, g_stuff_names[g_stuff_next[id]], "on_stuff_before");
}

public on_stuff_before(id, const cvar[], const value[])
{
	if (!is_user_connected(id))
		return;
	copy(g_before[id], charsmax(g_before[]), value);
	new k = g_stuff_next[id];
	client_cmd(id, "%s ^"%s^"", g_stuff_names[k], g_stuff_values[k]);
	client_cmd(id, "%s ^"%s^"", g_stuff_names[k], g_stuff_values[k]);
	set_task(0.3, "stuff_check", id);
}

public stuff_check(id)
{
	if (!is_user_connected(id))
		return;
	query_client_cvar(id, g_stuff_names[g_stuff_next[id]], "on_stuff_after");
}

public on_stuff_after(id, const cvar[], const value[])
{
	if (!is_user_connected(id))
		return;
	log_to_file("rs_bench.log", "S|%s|%s|%s", cvar, g_before[id], value);
	client_cmd(id, "%s ^"%s^"", cvar, g_before[id]);
	g_stuff_next[id]++;
	set_task(0.3, "stuff_next", id);
}

// ------------------------------------------------------------------ X phase

stock stuff_raw(id, const text[])
{
	message_begin(MSG_ONE, SVC_STUFFTEXT, _, id);
	write_string(text);
	message_end();
}

public on_x(id, const cvar[], const value[])
{
	log_to_file("rs_bench.log", "X|%s|%s", cvar, value);
}

// P1: two raw messages in one frame, a multi-command line, a quoted `;`, a mixed message with a
// refused command, and the allow_autoaim special case.
public x_p1(id)
{
	if (!is_user_connected(id))
		return;
	stuff_raw(id, "host_limitlocal 1^n");
	stuff_raw(id, "host_limitlocal 1^n");
	client_cmd(id, "cl_dlmax 1001;cl_lw 0");
	client_cmd(id, "setinfo _rsb ^"a;b^"");
	client_cmd(id, "cl_lc 0;bind F11 x");
	stuff_raw(id, "allow_autoaim 1^nrsb_marker^n");
	set_task(1.5, "x_p1_check", id);
}

public x_p1_check(id)
{
	if (!is_user_connected(id))
		return;
	query_client_cvar(id, "host_limitlocal", "on_x");
	query_client_cvar(id, "cl_dlmax", "on_x");
	query_client_cvar(id, "cl_lw", "on_x");
	query_client_cvar(id, "cl_lc", "on_x");
	new v[64];
	get_user_info(id, "_rsb", v, charsmax(v));
	log_to_file("rs_bench.log", "X|_rsb|%s", v);
	set_task(1.0, "x_p2", id);
}

// P2: engine commands that stay local, and an unknown one that must arrive verbatim.
public x_p2(id)
{
	if (!is_user_connected(id))
		return;
	client_cmd(id, "fullserverinfo ^"\a\b^"");
	client_cmd(id, "snapshot");
	client_cmd(id, "screenshot");
	client_cmd(id, "cmdlist");
	client_cmd(id, "version");
	client_cmd(id, "contimes");
	client_cmd(id, "clear");
	client_cmd(id, "rsb_unknown 1 ^"2 3^"");
	set_task(2.0, "x_p4", id);
}

// P4: long names and a mixed-case name. (A raw type-1 query's answer goes to pfnCvarValue, which
// this AMXX build does not expose.)
public x_p4(id)
{
	if (!is_user_connected(id))
		return;
	new name[256], len[1];
	for (new i = 0; i < 255; i++)
		name[i] = 'a';
	name[254] = 0;
	len[0] = 254;
	query_client_cvar(id, name, "on_x_len", 1, len);
	name[254] = 'a';
	name[255] = 0;
	len[0] = 255;
	query_client_cvar(id, name, "on_x_len", 1, len);
	query_client_cvar(id, "CL_LW", "on_x");
	set_task(1.5, "x_p6", id);
}

public on_x_len(id, const cvar[], const value[], const param[])
{
	log_to_file("rs_bench.log", "X|len%d|%s", param[0], value);
}

// P6: an http:// MOTD (run.py serves it on 127.0.0.1:8088 and logs the browser's requests).
public x_p6(id)
{
	if (!is_user_connected(id))
		return;
	new url[96];
	formatex(url, charsmax(url), "http://127.0.0.1:8088/rsb?uid=%d", get_user_userid(id));
	show_motd(id, url, "rsb");
	set_task(3.0, "x_p9", id);
}

// P9: a director stufftext (DRC_CMD_STUFFTEXT, type 10).
public x_p9(id)
{
	if (!is_user_connected(id))
		return;
	new text[] = "cl_dlmax 1003^n";
	message_begin(MSG_ONE, SVC_DIRECTOR, _, id);
	write_byte(strlen(text) + 2);
	write_byte(10);
	write_string(text);
	message_end();
	set_task(1.0, "x_p9_check", id);
}

public x_p9_check(id)
{
	if (!is_user_connected(id))
		return;
	query_client_cvar(id, "cl_dlmax", "on_x_director");
	set_task(0.5, "x_p10", id);
}

public on_x_director(id, const cvar[], const value[])
{
	log_to_file("rs_bench.log", "X|director_%s|%s", cvar, value);
}

// P10: bytes through setinfo and a string cvar must reach the server unchanged. ReHLDS refuses
// userinfo that is not UTF-8 ("Keys and values must be valid utf8 text"), so the cp1251 key must
// be refused (proof the raw bytes went out) and the UTF-8 one must arrive byte for byte.
public x_p10(id)
{
	if (!is_user_connected(id))
		return;
	client_cmd(id, "setinfo _rsb8 ^"%c%c%c^"", 0xC8, 0xE3, 0xF0);
	client_cmd(id, "setinfo _rsb9 ^"%c%c%c%c^"", 0xD0, 0x98, 0xD0, 0xB3);
	client_cmd(id, "cl_logofile ^"%c%c^"", 0xC8, 0xE3);
	set_task(1.0, "x_p10_check", id);
}

public x_p10_check(id)
{
	if (!is_user_connected(id))
		return;
	new v[64];
	get_user_info(id, "_rsb8", v, charsmax(v));
	log_to_file("rs_bench.log", "X|_rsb8|%02x%02x%02x", v[0] & 0xFF, v[1] & 0xFF, v[2] & 0xFF);
	get_user_info(id, "_rsb9", v, charsmax(v));
	log_to_file("rs_bench.log", "X|_rsb9|%02x%02x%02x%02x", v[0] & 0xFF, v[1] & 0xFF, v[2] & 0xFF, v[3] & 0xFF);
	query_client_cvar(id, "cl_logofile", "on_x_bytes");
}

public on_x_bytes(id, const cvar[], const value[])
{
	log_to_file("rs_bench.log", "X|bytes_%s|%02x%02x", cvar, value[0] & 0xFF, value[1] & 0xFF);
	client_cmd(id, "cl_logofile lambda");
	set_task(0.5, "x_menu", id);
}

// A ShowMenu longer than the client's 511-byte buffer, all two-byte UTF-8 (U+0416, D0 96): the cut at 511
// splits a character (the original keeps the lead byte; the client must survive it).
public x_menu(id)
{
	if (is_user_connected(id))
	{
		new text[701];
		for (new i = 0; i < 700; i += 2)
		{
			text[i] = 0xD0;
			text[i + 1] = 0x96;
		}
		text[700] = 0;
		show_menu(id, 1023, text, 5, "rsb_menu");
	}
	log_to_file("rs_bench.log", "XDONE");
}
