"""Writes the two synthetic CDD files of the tests: `mini-uds.cdd` and `mini-kwp.cdd`.

They are written for this project, small enough to read, and shaped like what CANdelaStudio
writes: a service is spread over three layers (instance, template, protocol) that point at each
other with `id` references. They cover every feature of the layout IR, so the tests do not depend
on Vector's samples. Run `python make_fixtures.py` to write them again.
"""
import sys
from xml.sax.saxutils import escape

LANGS = ("en-US", "de-DE")


class Ids:
    def __init__(self):
        self.n = 0x1000

    def new(self):
        self.n += 8
        return f"_0x{self.n:08x}"


def tuv(tag, texts):
    inner = "".join(f"<TUV xml:lang='{lang}'>{escape(t)}</TUV>" for lang, t in zip(LANGS, texts))
    return f"<{tag}>{inner}</{tag}>"


def names(en, de=None):
    return tuv("NAME", (en, de or en))


class Doc:
    def __init__(self, protocol, ecu, variant):
        self.ids = Ids()
        self.protocol, self.ecu, self.variant = protocol, ecu, variant
        self.datatypes, self.services, self.templates, self.classes, self.nrcs = [], [], [], [], {}
        self.dt = {}

    # ---- data types -------------------------------------------------------------------
    def _cv(self, bl, bo, enc, qty, minsz, maxsz):
        return f"<CVALUETYPE bl='{bl}' bo='{bo}' enc='{enc}' sig='0' df='hex' qty='{qty}' sz='no' minsz='{minsz}' maxsz='{maxsz}'/>"

    def ident(self, qual, bl, bo="21", enc="uns", qty="atom", minsz=1, maxsz=1):
        i = self.ids.new()
        pv = f"<PVALUETYPE bl='{bl}' bo='{bo}' enc='{enc}' sig='0' df='dec' qty='{qty}' sz='no' minsz='{minsz}' maxsz='{maxsz}'/>"
        self.datatypes.append(f"<IDENT id='{i}' bm='4294967295'>{names(qual)}<QUAL>{qual}</QUAL>{self._cv(bl, bo, enc, qty, minsz, maxsz)}{pv}</IDENT>")
        self.dt[qual] = i
        return i

    def texttbl(self, qual, bl, entries, bo="21"):
        i = self.ids.new()
        maps = "".join(f"<TEXTMAP s='{s}' e='{e}'><TEXT>{''.join(f'<TUV xml:lang={chr(39)}{l}{chr(39)}>{escape(t)}</TUV>' for l, t in zip(LANGS, (en, de)))}</TEXT></TEXTMAP>" for s, e, en, de in entries)
        pv = f"<PVALUETYPE bl='16' bo='21' enc='utf' sig='0' df='text' qty='field' sz='no' minsz='0' maxsz='65535'/>"
        self.datatypes.append(f"<TEXTTBL id='{i}' bm='4294967295'>{names(qual)}<QUAL>{qual}</QUAL>{self._cv(bl, bo, 'uns', 'atom', 1, 1)}{pv}{maps}</TEXTTBL>")
        self.dt[qual] = i
        return i

    def lincomp(self, qual, bl, comps, unit=None, enc="uns", bo="21"):
        i = self.ids.new()
        u = f"<UNIT>{unit}</UNIT>" if unit else ""
        pv = f"<PVALUETYPE bl='64' bo='21' enc='dbl' sig='1' df='flt' qty='atom' sz='no' minsz='0' maxsz='255'>{u}</PVALUETYPE>"
        cs = "".join("<COMP " + " ".join(f"{k}='{v}'" for k, v in c.items()) + "/>" for c in comps)
        self.datatypes.append(f"<LINCOMP id='{i}' bm='4294967295'>{names(qual)}<QUAL>{qual}</QUAL>{self._cv(bl, bo, enc, 'atom', 1, 1)}{pv}{cs}</LINCOMP>")
        self.dt[qual] = i
        return i

    def structdt(self, qual, members_xml, bl=8):
        i = self.ids.new()
        self.datatypes.append(f"<STRUCTDT id='{i}' bm='4294967295'>{names(qual)}<QUAL>{qual}</QUAL>{self._cv(bl, 21, 'uns', 'field', 1, 1)}{members_xml}</STRUCTDT>")
        self.dt[qual] = i
        return i

    def muxdt(self, qual, default_xml, cases):
        i = self.ids.new()
        body = "".join(f"<CASE s='{s}' e='{e}'><STRUCTURE>{x}</STRUCTURE></CASE>" for s, e, x in cases)
        self.datatypes.append(f"<MUXDT id='{i}' bm='4294967295'>{names(qual)}<QUAL>{qual}</QUAL>{self._cv(8, 12, 'uns', 'field', 1, 9)}<STRUCTURE>{default_xml}</STRUCTURE>{body}</MUXDT>")
        self.dt[qual] = i
        return i

    def nrc(self, v, qual, en):
        self.nrcs[v] = (self.ids.new(), qual, en)

    # ---- protocol layer ---------------------------------------------------------------
    def const(self, spec, bl, v, qual, resp_sup=False):
        i = self.ids.new()
        r = " respsupbit='1'" if resp_sup else ""
        return i, f"<CONSTCOMP id='{i}' must='1' spec='{spec}' bl='{bl}' v='{v}'{r}>{names(qual)}<QUAL>{qual}</QUAL></CONSTCOMP>"

    def static(self, spec, qual, bl=None, dtref=None, resp_sup=False):
        i = self.ids.new()
        r = " respsupbit='1'" if resp_sup else ""
        w = f" dtref='{dtref}'" if dtref else f" bl='{bl}'"
        return i, f"<STATICCOMP id='{i}' must='1' spec='{spec}'{w}{r}>{names(qual)}<QUAL>{qual}</QUAL></STATICCOMP>"

    def proxy(self, dest, qual, minbl=8, maxbl=None, tag="SIMPLEPROXYCOMP", extra=""):
        i = self.ids.new()
        mx = f" maxbl='{maxbl}'" if maxbl else ""
        return i, f"<{tag} id='{i}' must='1' dest='{dest}' minbl='{minbl}'{mx}{extra}>{names(qual)}<QUAL>{qual}</QUAL></{tag}>"

    def protocol_service(self, qual, sid, req, pos, neg_codes):
        """req/pos: lists of (id, xml). The negative response is the same for every service."""
        i = self.ids.new()
        _, sid_nr = self.const("sid", 8, 127, "SID_NR")
        _, sid_rq = self.const("sid", 8, sid, "SID_RQ_NR")
        rc_id, rc = self.proxy("resCode", "RC", 8, 8)
        nrc_refs = "".join(f"<NEGRESCODEPROXY idref='{self.nrcs[c][0]}'/>" for c in neg_codes)
        pos_xml = f"<POS>{names(qual + ' pos')}<QUAL>{qual}_PR</QUAL>{''.join(x for _, x in pos)}</POS>" if pos is not None else ""
        xml = (f"<PROTOCOLSERVICE id='{i}' func='0' phys='1' mresp='0' respOnPhys='1' respOnFunc='0'>{names(qual)}<QUAL>{qual}</QUAL>"
               f"<REQ>{names(qual + ' req')}<QUAL>{qual}_RQ</QUAL>{''.join(x for _, x in req)}</REQ>{pos_xml}"
               f"<NEG>{names(qual + ' neg')}<QUAL>{qual}_NR</QUAL>{sid_nr}{sid_rq}{rc}</NEG><NEGRESCODEPROXIES>{nrc_refs}</NEGRESCODEPROXIES></PROTOCOLSERVICE>")
        self.services.append(xml)
        return i

    # ---- template and instance layers -------------------------------------------------
    def instance(self, cls, inst, services, statics, contents, nrc_codes):
        """services: [(qual, protocol service id, [static comp ids], [proxy comp ids by content index])].

        One DCLTMPL per instance keeps the fixture simple: it holds a DCLSRVTMPL per service, a
        SHSTATIC per static value and a SHPROXY per content.
        """
        tmpl = self.ids.new()
        srv_tmpl, shs, shp, service_xml, value_xml, content_xml = [], [], [], [], [], []
        for qual, ps_id, _ in services:
            st = self.ids.new()
            srv_tmpl.append(f"<DCLSRVTMPL id='{st}' tmplref='{ps_id}' conv='req'><QUAL>{qual}</QUAL></DCLSRVTMPL>")
            si = self.ids.new()
            service_xml.append(f"<SERVICE id='{si}' tmplref='{st}' func='0' phys='1' mresp='0' respOnPhys='1' respOnFunc='0' req='0'>{names(qual)}<QUAL>{qual}</QUAL></SERVICE>")
        for spec, comp_ids, v in statics:
            sid = self.ids.new()
            shs.append(f"<SHSTATIC id='{sid}' spec='{spec}'><QUAL>S{sid[-4:]}</QUAL>{''.join(f'<STATICCOMPREF idref={chr(39)}{c}{chr(39)}/>' for c in comp_ids)}</SHSTATIC>")
            value_xml.append(f"<STATICVALUE shstaticref='{sid}' v='{v}'/>")
        for dest, comp_ids, data_xml in contents:
            pid = self.ids.new()
            shp.append(f"<SHPROXY id='{pid}' dest='{dest}'><QUAL>P{pid[-4:]}</QUAL>{''.join(f'<PROXYCOMPREF idref={chr(39)}{c}{chr(39)}/>' for c in comp_ids)}</SHPROXY>")
            content_xml.append(f"<SIMPLECOMPCONT shproxyref='{pid}'>{data_xml}</SIMPLECOMPCONT>")
        self.templates.append(f"<DCLTMPL id='{tmpl}' cls='dat' single='0'><QUAL>{inst}_T</QUAL>{''.join(srv_tmpl)}{''.join(shs)}{''.join(shp)}</DCLTMPL>")
        inst_xml = f"<DIAGINST id='{self.ids.new()}' tmplref='{tmpl}' req='0'>{names(inst)}<QUAL>{inst}</QUAL>{''.join(service_xml)}{''.join(value_xml)}{''.join(content_xml)}</DIAGINST>"
        self.classes.append((cls, inst_xml))

    def dataobj(self, qual, dtref, default=None):
        d = f" def='{default}'" if default is not None else ""
        return f"<DATAOBJ spec='no' dtref='{dtref}'{d}>{names(qual)}<QUAL>{qual}</QUAL></DATAOBJ>"

    def struct(self, qual, dtref, children):
        return f"<STRUCT spec='no' dtref='{dtref}'>{names(qual)}<QUAL>{qual}</QUAL>{children}</STRUCT>"

    def render(self):
        nrc_xml = "".join(f"<NEGRESCODE id='{i}' v='{v}'>{names(en)}<QUAL>{q}</QUAL></NEGRESCODE>" for v, (i, q, en) in sorted(self.nrcs.items()))
        classes = {}
        for cls, x in self.classes:
            classes.setdefault(cls, []).append(x)
        class_xml = "".join(f"<DIAGCLASS id='{self.ids.new()}' tmplref='{self.templates[0].split(chr(39))[1]}'><NAME><TUV xml:lang='en-US'>{c}</TUV></NAME><QUAL>{c}</QUAL>{''.join(xs)}</DIAGCLASS>" for c, xs in classes.items())
        return (
            "<?xml version='1.0' encoding='utf-8' standalone='no'?>\n<!DOCTYPE CANDELA SYSTEM 'candela.dtd'>\n"
            f"<CANDELA dtdvers='13.0.103'><ECUDOC doctype='inst' manufacturer='test' languages='({','.join(LANGS)})'>"
            f"<PROTOCOLSTANDARD>{self.protocol}</PROTOCOLSTANDARD>"
            f"<NEGRESCODES>{nrc_xml}</NEGRESCODES>"
            f"<DATATYPES>{''.join(self.datatypes)}</DATATYPES>"
            f"<PROTOCOLSERVICES>{''.join(self.services)}</PROTOCOLSERVICES>"
            f"<DCLTMPLS>{''.join(self.templates)}</DCLTMPLS>"
            f"<ECU id='{self.ids.new()}'>{names(self.ecu)}<QUAL>{self.ecu}</QUAL>"
            f"<VAR id='{self.ids.new()}' base='1'>{names(self.variant)}<QUAL>{self.variant}</QUAL>{class_xml}</VAR></ECU>"
            "</ECUDOC></CANDELA>\n"
        )


def common_nrcs(d):
    for v, q, en in [(0x11, "ServiceNotSupported", "Service not supported"), (0x12, "SubFunctionNotSupported", "Subfunction not supported"),
                     (0x13, "IncorrectMessageLength", "Incorrect message length or invalid format"), (0x22, "ConditionsNotCorrect", "Conditions not correct"),
                     (0x31, "RequestOutOfRange", "Request out of range"), (0x33, "SecurityAccessDenied", "Security access denied"),
                     (0x78, "ResponsePending", "Request correctly received, response pending")]:
        d.nrc(v, q, en)


def uds():
    d = Doc("UDS", "Mini", "Base")
    common_nrcs(d)
    u8 = d.ident("UnsignedDec_1Byte", 8)
    u16 = d.ident("UnsignedDec_2Byte", 16)
    u32le = d.ident("UnsignedDec_4Byte_LE", 32, bo="12")
    i16 = d.ident("SignedDec_2Byte", 16, enc="sgn")
    bcd = d.ident("Bcd_2Byte", 16, enc="bcd")
    f32 = d.ident("Float_4Byte", 32, enc="flt")
    did16 = d.ident("DataIdentifier", 16)
    vin = d.ident("Ascii_17Byte", 8, enc="asc", qty="field", minsz=17, maxsz=17)
    blob = d.ident("Blob_0_to_16", 8, qty="field", minsz=0, maxsz=16)
    onoff = d.texttbl("OffOn_1Byte", 8, [(0, 0, "off", "aus"), (1, 255, "on", "ein")])
    session = d.texttbl("SessionType", 8, [(1, 1, "default", "Standard"), (2, 2, "programming", "Programmierung"), (3, 3, "extended", "Erweitert")])
    volt = d.lincomp("Voltage_2Byte", 16, [{"f": "0.001", "o": "0"}], unit="V")
    temp = d.lincomp("Temperature_1Byte", 8, [{"s": "0", "e": "255", "f": "1", "o": "-40"}], unit="degC")
    bit1 = d.texttbl("Flag_1bit", 1, [(0, 0, "clear", "frei"), (1, 1, "set", "gesetzt")])
    mode3 = d.texttbl("Mode_3bit", 3, [(0, 0, "idle", "Leerlauf"), (1, 1, "run", "Lauf"), (7, 7, "fault", "Fehler")])
    s4 = d.ident("Signed_4bit", 4, enc="sgn")
    byte_container = d.ident("Container_1Byte", 8)

    # 10: DiagnosticSessionControl, with the suppress-positive-response bit.
    s_id, s_xml = d.static("sub", "SessionType", dtref=session, resp_sup=True)
    s2_id, s2_xml = d.static("sub", "SessionTypeEcho", dtref=session)
    p2_id, p2 = d.proxy("data", "Timing", 32, 32)
    _, c10 = d.const("sid", 8, 0x10, "SID_RQ")
    _, c50 = d.const("sid", 8, 0x50, "SID_PR")
    dsc = d.protocol_service("DSC", 0x10, [(0, c10), (s_id, s_xml)], [(0, c50), (s2_id, s2_xml), (p2_id, p2)], [0x12, 0x22, 0x13])
    timing_struct = d.dataobj("P2", u16) + d.dataobj("P2Star", u16)
    for name, v in [("Default", 1), ("Programming", 2), ("Extended", 3)]:
        d.instance("Sessions", f"{name}Session", [("Start", dsc, [])], [("sub", [s_id, s2_id], v)], [("data", [p2_id], timing_struct)], [0x12])

    # 3E: TesterPresent, a constant sub-function with the suppress bit.
    _, c3e = d.const("sid", 8, 0x3E, "SID_RQ")
    _, c7e = d.const("sid", 8, 0x7E, "SID_PR")
    _, sub0 = d.const("sub", 8, 0, "ZeroSubFunction", resp_sup=True)
    _, sub0p = d.const("sub", 8, 0, "ZeroSubFunction")
    tp = d.protocol_service("TP", 0x3E, [(0, c3e), (0, sub0)], [(0, c7e), (0, sub0p)], [0x13])
    d.instance("TesterPresent", "TesterPresent", [("Send", tp, [])], [], [], [0x13])

    # 22 / 2E: data by identifier.
    def data_service(name, did, data_xml, writable=True):
        # Request and response of ReadDataByIdentifier share one static component (the DID); the
        # positive response has its own.
        rq_id, rq_xml = d.static("id", "DataIdentifier", dtref=did16)
        rs_id, rs_xml = d.static("id", "DataIdentifier", dtref=did16)
        rp_id, rp_xml = d.proxy("data", "DataRecord", 8)
        _, c22 = d.const("sid", 8, 0x22, "SID_RQ")
        _, c62 = d.const("sid", 8, 0x62, "SID_PR")
        rdbi = d.protocol_service("RDBI_" + name, 0x22, [(0, c22), (rq_id, rq_xml)], [(0, c62), (rs_id, rs_xml), (rp_id, rp_xml)], [0x13, 0x31])
        services = [("Read", rdbi, [])]
        statics = [("id", [rq_id, rs_id], did)]
        contents = [("data", [rp_id], data_xml)]
        if writable:
            wq_id, wq_xml = d.static("id", "DataIdentifier", dtref=did16)
            wp_id, wp_xml = d.proxy("data", "DataRecord", 8)
            wr_id, wr_xml = d.static("id", "DataIdentifier", dtref=did16)
            _, c2e = d.const("sid", 8, 0x2E, "SID_RQ")
            _, c6e = d.const("sid", 8, 0x6E, "SID_PR")
            wdbi = d.protocol_service("WDBI_" + name, 0x2E, [(0, c2e), (wq_id, wq_xml), (wp_id, wp_xml)], [(0, c6e), (wr_id, wr_xml)], [0x13, 0x31, 0x33])
            services.append(("Write", wdbi, []))
            statics.append(("id", [wq_id, wr_id], did))
            contents.append(("data", [wp_id], data_xml))
        d.instance("Data", name, services, statics, contents, [0x13, 0x31])

    data_service("Vin", 0xF190, d.dataobj("Vin", vin))
    data_service("Battery", 0x0100, d.dataobj("Voltage", volt) + d.dataobj("Temperature", temp) + d.dataobj("Charging", onoff) + d.struct("Status", byte_container, d.dataobj("Alarm", bit1) + d.dataobj("Mode", mode3) + d.dataobj("Delta", s4)))
    data_service("Counters", 0x0200, d.dataobj("Total", u32le) + d.dataobj("Offset", i16) + d.dataobj("Serial", bcd) + d.dataobj("Gain", f32))
    data_service("Blob", 0x0300, d.dataobj("Bytes", blob))
    coding = d.structdt("CodingRecord", d.dataobj("Country", onoff) + d.dataobj("Limit", volt))
    data_service("Coding", 0x0400, d.dataobj("Coding", coding))

    # 19 02: ReadDTCInformation, the answer is a list that runs to the end of the message.
    mask_id, mask = d.proxy("dtcStatus", "StatusMask", 8, 8, tag="STATUSDTCPROXYCOMP")
    avail_id, avail = d.proxy("dtcStatus", "AvailabilityMask", 8, 8, tag="STATUSDTCPROXYCOMP")
    dtc_id, dtc = d.proxy("dtc", "DTC", 24, 24)
    st_id, st = d.proxy("dtcStatus", "StatusOfDTC", 8, 8, tag="STATUSDTCPROXYCOMP")
    it_id = d.ids.new()
    iteration = f"<EOSITERCOMP id='{it_id}' must='1' minNumOfItems='0'>{names('ListOfDTC')}<QUAL>ListOfDTC</QUAL>{dtc}{st}</EOSITERCOMP>"
    _, c19 = d.const("sid", 8, 0x19, "SID_RQ")
    _, c59 = d.const("sid", 8, 0x59, "SID_PR")
    _, r02 = d.const("sub", 8, 2, "ReportDtcByStatusMask", resp_sup=True)
    _, r02p = d.const("sub", 8, 2, "ReportDtcByStatusMask")
    rdtc = d.protocol_service("RDTCI", 0x19, [(0, c19), (0, r02), (mask_id, mask)], [(0, c59), (0, r02p), (avail_id, avail), (it_id, iteration)], [0x12, 0x13])
    dtc_table = d.ident("DtcNumber", 24)
    status_byte = d.ident("DtcStatus", 8)
    d.instance("FaultMemory", "FaultMemory", [("ReadByStatusMask", rdtc, [])], [], [
        ("dtcStatus", [mask_id], d.dataobj("StatusMask", status_byte)),
        ("dtcStatus", [avail_id], d.dataobj("AvailabilityMask", status_byte)),
        ("dtc", [dtc_id], d.dataobj("DTC", dtc_table)),
        ("dtcStatus", [st_id], d.dataobj("StatusOfDTC", status_byte)),
    ], [0x12])

    # 23: ReadMemoryByAddress, a multiplexer: the format byte says how long address and size are.
    a1, a2, s1, s2 = d.ident("Addr_1Byte", 8), d.ident("Addr_2Byte", 16), d.ident("Size_1Byte", 8), d.ident("Size_2Byte", 16)
    mux = d.muxdt("MemoryAddressAndSize", d.dataobj("Address", a1) + d.dataobj("Size", s1), [
        (17, 17, d.dataobj("Address", a1) + d.dataobj("Size", s1)),
        (34, 34, d.dataobj("Address", a2) + d.dataobj("Size", s2)),
        (18, 18, d.dataobj("Address", a2) + d.dataobj("Size", s1)),
    ])
    alfid = d.ident("AddressAndLengthFormat", 8)
    ma_id, ma = d.proxy("data", "Request", 8)
    data_id, data = d.proxy("data", "DataRecord", 8)
    _, c23 = d.const("sid", 8, 0x23, "SID_RQ")
    _, c63 = d.const("sid", 8, 0x63, "SID_PR")
    rmba = d.protocol_service("RMBA", 0x23, [(0, c23), (ma_id, ma)], [(0, c63), (data_id, data)], [0x13, 0x31, 0x33])
    selector_obj = f"<DATAOBJ id='{d.ids.new()}' spec='no' def='17' dtref='{alfid}'>{names('Format')}<QUAL>Format</QUAL></DATAOBJ>"
    sel_id = selector_obj.split("'")[1]
    mux_obj = f"<DATAOBJ spec='no' dtref='{mux}' dataObjectRef='{sel_id}'>{names('AddressAndSize')}<QUAL>AddressAndSize</QUAL></DATAOBJ>"
    d.instance("Memory", "Memory", [("Read", rmba, [])], [], [("data", [ma_id], selector_obj + mux_obj), ("data", [data_id], d.dataobj("Memory", blob))], [0x13, 0x31])
    return d


def kwp():
    d = Doc("KWP", "MiniKwp", "Base")
    common_nrcs(d)
    bcd2 = d.ident("Bcd_2Byte", 16, enc="bcd")
    u8 = d.ident("UnsignedDec_1Byte", 8)
    u16 = d.ident("UnsignedDec_2Byte", 16)
    group = d.ident("GroupOfDtc", 16)
    dtc_number = d.ident("DtcNumber", 16)
    onoff = d.texttbl("OffOn_1Byte", 8, [(0, 0, "off", "aus"), (1, 255, "on", "ein")])
    bit1 = d.texttbl("Flag_1bit", 1, [(0, 0, "false", "falsch"), (1, 1, "true", "wahr")])
    status_container = d.ident("StatusContainer_1Byte", 8)
    status = d.structdt("DtcStatusByte", d.struct("Bits", status_container, d.dataobj("Confirmed", bit1) + d.dataobj("NotCompleted", bit1)))

    # 1A: ReadECUIdentification, a local identifier and a few BCD numbers.
    o_id, o = d.static("lid", "OPTION", bl=8)
    io_id, io = d.static("lid", "IO", bl=8)
    data_id, data = d.proxy("data", "DATA", 8)
    _, c1a = d.const("sid", 8, 0x1A, "SID_RQ")
    _, c5a = d.const("sid", 8, 0x5A, "SID_PR")
    rei = d.protocol_service("REI", 0x1A, [(0, c1a), (o_id, o)], [(0, c5a), (io_id, io), (data_id, data)], [0x12, 0x13])
    d.instance("Identification", "EcuIdentification", [("Read", rei, [])], [("lid", [o_id, io_id], 0x90)], [("data", [data_id], d.dataobj("IdentNumber_High", bcd2) + d.dataobj("IdentNumber_Low", bcd2) + d.dataobj("Identification", u16))], [0x12])

    # 18: ReadDTCByStatus, answered with a number and that many (DTC, status) pairs.
    opt_id, opt = d.const("sub", 8, 2, "OptionAllIdentified")
    grp_id, grp = d.proxy("groupOfDtc", "GroupOfDtc", 16, 16, tag="GROUPOFDTCPROXYCOMP", extra=f" dtref='{group}'")
    num_id, num = d.proxy("any", "NumberOfDtc", 8, 8)
    dtc_id, dtc = d.proxy("dtc", "DTC", 16, 16)
    st_id, st = d.proxy("dtcStatus", "DtcStatusByte", 8, 8, tag="STATUSDTCPROXYCOMP")
    it_id = d.ids.new()
    iteration = f"<NUMITERCOMP id='{it_id}' must='1' selref='{num_id}' selbm='4294967295'>{names('ListOfDtc')}<QUAL>ListOfDtc</QUAL>{dtc}{st}</NUMITERCOMP>"
    _, c18 = d.const("sid", 8, 0x18, "SID_RQ")
    _, c58 = d.const("sid", 8, 0x58, "SID_PR")
    _, sub2 = d.const("sub", 8, 2, "OptionAllIdentified")
    rdtc = d.protocol_service("RDTCBS", 0x18, [(0, c18), (opt_id, opt), (grp_id, grp)], [(0, c58), (num_id, num), (it_id, iteration)], [0x12, 0x13])
    d.instance("FaultMemory", "FaultMemory", [("ReadAllIdentified", rdtc, [])], [], [
        ("dtc", [dtc_id], d.dataobj("DTC", dtc_number)),
        ("dtcStatus", [st_id], d.dataobj("DtcStatusByte", status)),
    ], [0x12])
    return d


if __name__ == "__main__":
    import os
    here = os.path.dirname(os.path.abspath(__file__))
    for name, make in (("mini-uds.cdd", uds), ("mini-kwp.cdd", kwp)):
        with open(os.path.join(here, name), "w", encoding="utf-8", newline="\n") as f:
            f.write(make().render())
        print("wrote", name)
