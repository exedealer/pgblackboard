with col as (
  select * from rows from (
    pg_catalog.json_to_recordset($1) as (
      "name" text,
      "table_oid" oid,
      "table_col" int2,
      "type_oid" oid,
      "type_mod" int4
    )
  ) with ordinality
),
geom_typoid as (
  select distinct prorettype
  from pg_proc
  where proname in ('st_geomfromtext', 'st_geogfromtext')
)
select pg_catalog.convert_to(res_jsonl, 'UTF8')
from (
  select col."table_oid", conkey
  from col
  -- TODO check unique index instead? so we can support tables with no pk
  join pg_catalog.pg_constraint on conrelid = col."table_oid" and contype = 'p'
  group by col."table_oid", conkey
  having conkey operator(pg_catalog.<@) pg_catalog.array_agg(col."table_col") filter (where col."table_col" > 0)
  union all
  select null, null
  order by 1 nulls last
  limit 1
) _ (target_reloid, target_key)

, pg_catalog.jsonb_build_object(
  'rel_name', (
    select pg_catalog.format('%I.%I', nspname, relname)
    from pg_catalog.pg_class
    join pg_catalog.pg_namespace on pg_namespace.oid = relnamespace
    where pg_class.oid = target_reloid
  ),
  'cols', array(
    select pg_catalog.jsonb_build_object(
      'name', col."name",
      -- TODO full qualified type name
      -- https://github.com/postgres/postgres/blob/064e04008533b2b8a82b5dbff7da10abd6e41565/src/backend/utils/adt/format_type.c#L60
      'type', pg_catalog.format_type(col."type_oid", col."type_mod"),
      'att_name', pg_catalog.quote_ident(attname),
      'att_key', attnum = any(target_key),
      'att_notnull', attnotnull,
      'is_geom', col."type_oid" in (table geom_typoid)
    )
    from col
    left join pg_catalog.pg_attribute on
      (attrelid, attnum) = (col."table_oid", col."table_col")
      and attrelid = target_reloid
    order by ordinality
  )
) res_json

-- We need no \n for our jsonl stream,
-- I found no guarantees in postgres documentation.
, pg_catalog.replace(res_json::text, e'\n', '') res_jsonl
