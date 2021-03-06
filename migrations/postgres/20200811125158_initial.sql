create table domain
(
	id char(32) not null
		constraint domain_pk
			primary key,
	username char(32) not null,
	password char(60) not null,
	txt varchar
);

create unique index domain_id_uindex
	on domain (id);

create unique index domain_password_uindex
	on domain (password);

create unique index domain_username_uindex
	on domain (username);


create table cert
(
	id char(32) not null
		constraint cert_pk
			primary key,
	update bigint not null,
	state integer not null,
	cert varchar,
    private varchar,
	domain_id char(32) not null
		constraint domain
			references domain
				on delete cascade
);

create unique index cert_id_uindex
	on cert (id);

create unique index cert_cert_uindex
	on cert (domain_id);


create table acme
(
	key varchar not null,
	realm bigint not null,
	kind varchar not null,
	value bytea not null
);

--create unique index acme_key_uindex
	--on acme (key);

--alter table acme
	--add constraint acme_pk
		--primary key (key);