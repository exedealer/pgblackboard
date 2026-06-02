  select coalesce(json_agg(it order by ord, "name"), '[]') from (

select text ''          db
  , text ''             ntype
  , oid '0'             noid
  , text ''             ntid
  , text '' collate "C" "name"
  , text ''             descr
  , text ''             mod
  , float8 '0'          size
  , bool 'false'        leaf
  , int2 '0'            ord
where false

-- databases
union all
select datname, 'database', null, null, datname, descr, null, null, false, 1
from pg_database, shobj_description(oid, 'pg_database') descr
where $1 is null
  and not datistemplate

-- schemas
union all
select current_catalog, 'schema', oid, null, nspname, descr, null, null, false, 1
from pg_namespace, obj_description(oid, 'pg_namespace') descr
where 'database' = $1
  and nspname not like all ('{ pg\\_toast , pg\\_temp\\_% , pg\\_toast\\_temp\\_% }')

-- functions
union all
select current_catalog, 'function', p.oid, null, sign, descr, mod, null, true, 2
from pg_proc p
left join pg_aggregate agg on aggfnoid = oid
, obj_description(p.oid, 'pg_proc') descr
, pg_get_function_result(p.oid) fnres
, format(
  '%s (%s)%s'
  , proname
  , pg_get_function_identity_arguments(p.oid)
  , e' \u2022 ' || fnres
) sign
,concat_ws(' '
  , case when agg is not null then 'aggregate' end
  , case when fnres is null then 'procedure' end
) mod
where ('schema', pronamespace) = ($1, $2)

-- tables
union all
select current_catalog, 'table', oid, null, relname, descr, mod, size, false, 1
from pg_class
, obj_description(oid, 'pg_class') descr
, format('table_%s', relkind) mod
, nullif(reltuples, -1) size
where ('schema', relnamespace) = ($1, $2)
  and relkind not in ('i', 'I', 't', 'c', 'S')

-- columns
union all
select current_catalog, 'column', attrelid, text(attnum), attname, descr, mod, null, true, attnum
from pg_attribute
, concat_ws(' '
  , format_type(atttypid, atttypmod)
  , case when attnotnull then 'not null' end
  , '-- ' || col_description(attrelid, attnum)
) descr
, concat_ws(' '
  , (
    select 'column_pk'
    from pg_constraint
    where conrelid = attrelid and contype = 'p' and attnum = any(conkey)
    -- limit 1
  )
) mod
where ('table', attrelid) = ($1, $2)
  and (attnum > 0 or attname = 'oid')
  and not attisdropped

-- constraints
union all
select current_catalog, 'constraint', oid, null, conname, descr, mod, null, true, 10010
from pg_constraint
, obj_description(oid, 'pg_constraint') descr
, concat_ws(' '
  , format('constraint_%s', contype)
  , case when not convalidated then 'constraint_not_validated' end
) mod
where ('table', conrelid) = ($1, $2)
  -- and contype not in ()

-- indexes
union all
select current_catalog, 'index', indexrelid, null, relname, descr, mod, null, true, 10020
from pg_index join pg_class on indexrelid = oid
, obj_description(indexrelid, 'pg_class') descr
, concat_ws(' ', null) mod -- TODO uniq
where ('table', indrelid) = ($1, $2)

-- triggers
union all
select current_catalog, 'trigger', oid, null, tgname, descr, mod, null, true, 10030
from pg_trigger
, obj_description(oid, 'pg_trigger') descr
, concat_ws(' ', null) mod -- TODO tgenabled=D
where ('table', tgrelid) = ($1, $2)
  and tgconstraint = 0

-- fs
union all
select current_catalog, 'dir', null, '.', './', null, null, null, false, 2
where $1 is null
  and has_function_privilege('pg_ls_dir(text)', 'execute')

-- dir
union all
select current_catalog, 'dir', null, fpath, fname, null, null, null, false, 1
from pg_ls_dir($3) fname
, concat($3, '/', fname) fpath
, pg_stat_file(fpath) stat
where 'dir' = $1
  and stat.isdir

-- file
union all
select current_catalog, 'file', null, fpath, fname, null, null, null, true, 2
from pg_ls_dir($3) fname
, concat($3, '/', fname) fpath
, pg_stat_file(fpath) stat
where 'dir' = $1
  and not stat.isdir

-- order by ord, "name"

  ) it
