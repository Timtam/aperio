//! The order a group names its cities in and the city it falls back to
//! (DESIGN-series-time-zone.md, 118a and the stage 11 decisions): written by
//! hand, group by group, against the groups tzdata 2026f forms. Only the order
//! within a group matters. A widely known, large city comes first, a capital
//! before another city of its country, and a long-stable name before one that
//! was renamed or changed lately (Phoenix before Vancouver, Mexico City before
//! Edmonton, Bogota before Winnipeg, Athens before Kyiv, which old Android
//! zone data lacks). A zone missing here ranks after every zone that is here,
//! by how many tzdata names resolve to it, then by city.
//!
//! Every entry is a listed zone in its exact spelling, once: a tzdata update
//! that turns one into a link fails the `table` test by name. The fixture
//! names the main city of every group, so a new or reshaped group shows too.

/// Listed zones, best first. One line per group as tzdata 2026f forms them.
#[rustfmt::skip]
pub(super) const PROMINENCE: &[&str] = &[
    "Pacific/Pago_Pago", "Pacific/Niue",
    "Pacific/Honolulu", "Pacific/Tahiti", "Pacific/Rarotonga",
    "America/Anchorage", "America/Juneau", "America/Sitka",
    "America/Los_Angeles", "America/Tijuana",
    "America/Denver", "America/Boise", "America/Ciudad_Juarez",
    "America/Phoenix", "America/Vancouver", "America/Hermosillo", "America/Mazatlan", "America/Whitehorse",
    "America/Mexico_City", "America/Guatemala", "America/Costa_Rica", "America/Monterrey", "America/El_Salvador", "America/Tegucigalpa", "America/Managua", "America/Regina", "America/Edmonton",
    "America/Chicago", "America/Matamoros", "America/Indiana/Knox",
    "America/Bogota", "America/Lima", "America/Panama", "America/Jamaica", "America/Guayaquil", "America/Cancun", "America/Winnipeg",
    "America/New_York", "America/Toronto", "America/Detroit", "America/Indiana/Indianapolis", "America/Port-au-Prince",
    "America/Caracas", "America/Santo_Domingo", "America/Puerto_Rico", "America/La_Paz", "America/Manaus",
    "America/Halifax", "Atlantic/Bermuda", "America/Moncton",
    "America/Sao_Paulo", "America/Argentina/Buenos_Aires", "America/Montevideo", "America/Asuncion",
    "America/Noronha", "Atlantic/South_Georgia",
    "America/Nuuk", "America/Scoresbysund",
    "Africa/Abidjan", "Africa/Casablanca", "Africa/Monrovia", "Africa/Bissau",
    "Europe/London", "Europe/Dublin", "Europe/Lisbon",
    "Africa/Lagos", "Africa/Algiers", "Africa/Tunis",
    "Europe/Berlin", "Europe/Paris", "Europe/Rome", "Europe/Madrid", "Europe/Vienna", "Europe/Warsaw", "Europe/Brussels", "Europe/Zurich", "Europe/Prague", "Europe/Budapest",
    "Africa/Johannesburg", "Africa/Khartoum", "Africa/Maputo", "Africa/Tripoli", "Africa/Windhoek",
    "Europe/Athens", "Europe/Helsinki", "Europe/Kyiv", "Europe/Bucharest", "Europe/Sofia",
    "Asia/Gaza", "Asia/Hebron",
    "Europe/Moscow", "Europe/Istanbul", "Asia/Riyadh", "Asia/Baghdad", "Africa/Nairobi", "Europe/Minsk",
    "Asia/Dubai", "Asia/Baku", "Asia/Tbilisi", "Asia/Yerevan", "Europe/Samara",
    "Asia/Karachi", "Asia/Tashkent", "Asia/Almaty", "Asia/Yekaterinburg", "Indian/Maldives",
    "Asia/Kolkata", "Asia/Colombo",
    "Asia/Dhaka", "Asia/Bishkek", "Asia/Omsk",
    "Asia/Jakarta", "Asia/Bangkok", "Asia/Ho_Chi_Minh", "Asia/Novosibirsk",
    "Asia/Shanghai", "Asia/Singapore", "Asia/Hong_Kong", "Asia/Taipei", "Asia/Manila", "Australia/Perth",
    "Asia/Tokyo", "Asia/Seoul", "Asia/Pyongyang", "Asia/Yakutsk",
    "Australia/Adelaide", "Australia/Broken_Hill",
    "Australia/Sydney", "Australia/Melbourne", "Australia/Hobart",
    "Australia/Brisbane", "Asia/Vladivostok", "Pacific/Port_Moresby", "Pacific/Guam",
    "Pacific/Noumea", "Pacific/Guadalcanal", "Asia/Sakhalin", "Asia/Magadan",
    "Pacific/Fiji", "Asia/Kamchatka", "Pacific/Tarawa",
    "Pacific/Apia", "Pacific/Tongatapu", "Pacific/Kanton",
];
