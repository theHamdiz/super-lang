# Law mutation report (0.1.0)

Generated: 2026-11-27T20:57:18.360378+00:00
Seed: `0xa1c0b26` (`SUPER_LAW_MUTATION_SEED`)

| Outcome | Count |
| --- | ---: |
| killed | 145 |
| survived | 11 |
| skipped | 11 |
| error | 0 |

**Mutation score: 92.9%** (145/156). PASS (≥70% overall)

## Modules

- `stdlib/core/bytes.sr` — killed=7 survived=1 — survived: flip_sub on line 347 (fn=bytes_last_index); killed: flip_add on line 347 (fn=bytes_last_index); killed: flip_sub on line 345 (fn=bytes_last_index); killed: flip_add on line 342 (fn=bytes_last_index); killed: flip_gt on line 345 (fn=bytes_last_index)
- `stdlib/core/collections.sr` — killed=7 survived=1 — killed: flip_sub on line 51 (fn=i_sdiv); killed: flip_add on line 635 (fn=popcount32); killed: flip_add on line 622 (fn=word_has); killed: flip_gt on line 637 (fn=popcount32); killed: flip_add on line 634 (fn=popcount32)
- `stdlib/core/decimal.sr` — killed=3 survived=0 — killed: flip_add on line 30 (fn=dec_abs); killed: flip_sub on line 31 (fn=dec_abs); killed: flip_lt on line 31 (fn=dec_abs)
- `stdlib/core/int.sr` — killed=1 survived=0 — killed: flip_add on line 35 (fn=is_u32)
- `stdlib/core/reduce.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/core/str.sr` — killed=2 survived=0 — killed: flip_add on line 384 (fn=starts_with); killed: flip_add on line 25 (fn=byte_len)
- `stdlib/core/unicode.sr` — killed=2 survived=0 — killed: flip_add on line 89 (fn=utf8_is_cont); killed: flip_eq on line 89 (fn=utf8_is_cont)
- `stdlib/data/column.sr` — killed=5 survived=1 — killed: flip_add on line 65 (fn=dt_same); killed: flip_sub on line 60 (fn=dt_abs); killed: flip_lt on line 65 (fn=dt_same); killed: flip_lt on line 60 (fn=dt_abs); survived: flip_gt on line 65 (fn=dt_same)
- `stdlib/data/expr.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/data/plan.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/datetime/core.sr` — killed=7 survived=1 — killed: flip_add on line 14 (fn=days_from_civil); killed: flip_add on line 45 (fn=is_leap_year); killed: flip_sub on line 21 (fn=days_from_civil); killed: flip_add on line 21 (fn=days_from_civil); killed: flip_sub on line 19 (fn=days_from_civil)
- `stdlib/datetime/cron.sr` — killed=7 survived=1 — killed: flip_eq on line 138 (fn=cron_match); killed: flip_add on line 132 (fn=cron_digit_acc); survived: flip_eq on line 147 (fn=cron_match); killed: flip_gt on line 133 (fn=cron_digit_acc); killed: flip_lt on line 133 (fn=cron_digit_acc)
- `stdlib/db/sql.sr` — killed=5 survived=0 — killed: flip_eq on line 272 (fn=sql_digit); killed: flip_add on line 269 (fn=sql_digit); killed: flip_sub on line 270 (fn=sql_digit); killed: flip_add on line 326 (fn=sql_bool); killed: flip_lt on line 272 (fn=sql_digit)
- `stdlib/diag/log.sr` — killed=2 survived=0 — killed: flip_add on line 313 (fn=field); killed: flip_add on line 315 (fn=field_int)
- `stdlib/fmt/bin.sr` — killed=2 survived=0 — killed: flip_lt on line 177 (fn=be32_get); killed: flip_add on line 177 (fn=be32_get)
- `stdlib/fmt/compress.sr` — killed=4 survived=1 — killed: flip_add on line 35 (fn=compress_xor32); killed: flip_lt on line 30 (fn=compress_u32); survived: flip_add on line 30 (fn=compress_u32); killed: flip_add on line 40 (fn=compress_xor32); killed: flip_add on line 28 (fn=compress_u32)
- `stdlib/fmt/csv.sr` — killed=8 survived=0 — killed: flip_eq on line 46 (fn=csv_is_lf); killed: flip_ne on line 47 (fn=csv_should_keep_char); killed: flip_eq on line 50 (fn=csv_is_quote); killed: flip_add on line 46 (fn=csv_is_lf); killed: flip_add on line 58 (fn=csv_parse)
- `stdlib/fmt/json.sr` — killed=4 survived=0 — killed: flip_add on line 162 (fn=json_is_ws); killed: flip_add on line 165 (fn=json_is_ws_b); killed: flip_eq on line 162 (fn=json_is_ws); killed: flip_eq on line 165 (fn=json_is_ws_b)
- `stdlib/fmt/msgpack.sr` — killed=4 survived=0 — killed: flip_eq on line 198 (fn=mp_payload); killed: flip_eq on line 197 (fn=mp_payload); killed: flip_eq on line 196 (fn=mp_payload); killed: flip_add on line 195 (fn=mp_payload)
- `stdlib/fmt/protobuf.sr` — killed=7 survived=0 — killed: flip_sub on line 66 (fn=pb_zigzag); killed: flip_add on line 70 (fn=pb_unzigzag); killed: flip_add on line 65 (fn=pb_zigzag); killed: flip_gt on line 70 (fn=pb_unzigzag); killed: flip_sub on line 70 (fn=pb_unzigzag)
- `stdlib/fmt/toml.sr` — killed=5 survived=0 — killed: flip_eq on line 277 (fn=toml_digit); killed: flip_add on line 273 (fn=toml_digit); killed: flip_eq on line 141 (fn=toml_is_space); killed: flip_sub on line 275 (fn=toml_digit); killed: flip_add on line 141 (fn=toml_is_space)
- `stdlib/fmt/xml.sr` — killed=5 survived=3 — survived: flip_eq on line 215 (fn=xml_char_ok); killed: flip_eq on line 276 (fn=xml_is_ws); killed: flip_eq on line 207 (fn=xml_char_ok); survived: flip_eq on line 214 (fn=xml_char_ok); survived: flip_eq on line 279 (fn=xml_is_ws)
- `stdlib/fmt/yaml.sr` — killed=3 survived=0 — killed: flip_eq on line 46 (fn=yaml_is_ws); killed: flip_eq on line 47 (fn=yaml_is_ws); killed: flip_add on line 45 (fn=yaml_is_ws)
- `stdlib/html/core.sr` — killed=7 survived=1 — killed: flip_eq on line 22 (fn=escape_text_entity); survived: flip_eq on line 23 (fn=escape_text_entity); killed: flip_add on line 19 (fn=escape_text_entity); killed: flip_add on line 15 (fn=escape_text_entity_quot); killed: flip_gt on line 22 (fn=escape_text_entity)
- `stdlib/html/parse.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/html/template.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/io/buf.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/math/core.sr` — killed=8 survived=0 — killed: flip_sub on line 203 (fn=m_smoothstep); killed: flip_sub on line 202 (fn=m_smoothstep); killed: flip_lt on line 41 (fn=m_min); killed: flip_add on line 201 (fn=m_smoothstep); killed: flip_sub on line 207 (fn=m_smootherstep)
- `stdlib/net/dns.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/net/http.sr` — killed=8 survived=0 — killed: flip_add on line 360 (fn=http_is_space_tab); killed: flip_add on line 363 (fn=http_trim_advance_lo); killed: flip_eq on line 266 (fn=http_is_token); killed: flip_add on line 264 (fn=http_is_token); killed: flip_lt on line 364 (fn=http_trim_advance_lo)
- `stdlib/net/url.sr` — killed=7 survived=1 — survived: flip_add on line 272 (fn=url_hex_byte); killed: flip_sub on line 222 (fn=url_last_index_of); killed: flip_lt on line 214 (fn=url_index_of); killed: flip_add on line 220 (fn=url_last_index_of); killed: flip_sub on line 212 (fn=url_index_of)
- `stdlib/sec/argon2.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/sec/csprng.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/sec/encoding.sr` — killed=8 survived=0 — killed: flip_lt on line 240 (fn=enc_hex_value); killed: flip_sub on line 233 (fn=enc_hex_value); killed: flip_eq on line 235 (fn=enc_hex_value); killed: flip_eq on line 240 (fn=enc_hex_value); killed: flip_lt on line 235 (fn=enc_hex_value)
- `stdlib/sec/hash.sr` — killed=4 survived=0 — killed: flip_lt on line 51 (fn=hash_u32); killed: flip_add on line 51 (fn=hash_u32); killed: flip_add on line 56 (fn=hash_xor32); killed: flip_add on line 49 (fn=hash_u32)
- `stdlib/sec/secret.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/sec/sha2.sr` — killed=0 survived=0 — skipped: no safe mutation site
- `stdlib/sys/path.sr` — killed=3 survived=0 — killed: flip_add on line 24 (fn=path_is_abs); killed: flip_eq on line 24 (fn=path_is_abs); killed: flip_ne on line 24 (fn=path_is_abs)
- `stdlib/sys/platform.sr` — killed=2 survived=0 — killed: flip_add on line 15 (fn=readiness_backend); killed: flip_add on line 16 (fn=has_io_uring)
- `stdlib/sys/time.sr` — killed=8 survived=0 — killed: flip_gt on line 352 (fn=time_days_from_civil); killed: flip_sub on line 355 (fn=time_days_from_civil); killed: flip_sub on line 350 (fn=time_days_from_civil); killed: flip_add on line 345 (fn=time_days_from_civil); killed: flip_add on line 353 (fn=time_days_from_civil)

