# TaintRadar: Semantic-Aware Taint-Style Vulnerability Detection via Augmented Code Property Graphs

- **作者**: Elie Rizk, Firas Ben Hmida, Birhanu Eshete
- **发表时间**: 2026-07-17T18:55:46Z
- **来源/期刊**: arXiv
- **来源标识**: 2607.16456
- **转换方式**: PDF 文本提取（公式可能退化，如有数学符号异常请见谅）

## 摘要

Despite significant advances, static vulnerability analysis suffers from three critical limitations: coarse sanitization modeling, which treats validation as a binary barrier; database blindness, which breaks taint tracking across persistence layers; and shallow object-oriented analysis, which misses field-level and interprocedural data flows. These flaws stem from a common root cause: Code Property Graph (CPG)-based taint analyses lack a compositional semantic layer to jointly model sanitization, persistence, and object aliasing. Consequently, existing tools generate excessive false positives or miss critical attack paths entirely. To address these limitations, we present TaintRadar, an approach that systematically augments CPGs with three semantic analysis layers. First, vulnerability-typed sanitization computes node-level safety guarantees using transfer functions and context-sensitive parameter binding. Second, persistence-aware propagation integrates database schema constraints and query safety analysis to track multi-script attack paths traversing shared database states. Finally, object-aware reaching definitions combine calling and bounded variable alias contexts to precisely model object-field mutations across method boundaries. We evaluate TaintRadar on both synthetic benchmarks and real-world systems. On the SARD benchmark, TaintRadar drastically reduces false positives while maintaining 80% overall accuracy. Deployed across 19 real-world PHP applications, it rediscovered the majority of known CVEs and uncovered 29 confirmed zero-day vulnerabilities, including 26 SQL injection and 3 stored XSS vulnerabilities, that have already received CVE identifiers. These results demonstrate that semantic-aware graph augmentation significantly improves the precision, coverage, and practical utility of static taint analysis.

## 正文

<!-- page 1 -->

1
TaintRadar: Semantic-Aware Taint-Style
Vulnerability Detection via Augmented Code
Property Graphs
Elie Rizk∗, Firas Ben Hmida†, Birhanu Eshete ‡,
∗elirizk@umich.edu, †fbhmida@umich.edu, ‡birhanu@umich.edu
Department of Computer and Information Science,
University of Michigan-Dearborn, Michigan, USA
Abstract—Despite significant advances, static vulnerability existingtechniques[1],[2],[3],[4],[5],[6],[7],[8],[9],[10],
analysissuffersfromthreecriticallimitations:coarsesanitization [11]faceseverearchitecturallimitations.Manypriorsolutions
modeling (treating validation as a binary barrier), database
remain narrow in scope, relying on coarse syntactic pat-
blindness (breaking taint tracking across persistence layers),
tern matching or naively tracing source-to-sink paths without
and shallow object-oriented analysis (missing field-level and
interprocedural data flows). These flaws stem from a common robust, context-aware verification, which triggers prohibitive
root cause: Code Property Graph (CPG)-based taint analyses false positive rates. Crucially, current CPG frameworks lack
lackacompositionalsemanticlayertojointlymodelsanitization, theabilitytomodelorenforcethesemanticoperationsgovern-
persistence, and object aliasing. Consequently, existing tools
ing how data is transformed between decoupled program lay-
generate excessive false positives or miss critical attack paths
ers. Object-oriented features and persistent database states are
entirely.
To address these limitations, we present TaintRadar, an frequently abstracted away or completely ignored [12], break-
approach that systematically augments CPGs with three in- ing data flow continuity. While hybrid approaches attempt
dependent semantic analysis layers. First, vulnerability-typed to address these gaps via dynamic execution engines [13]
sanitization computes node-level safety guarantees using trans- or localized call-graph refinements [14], they fail to capture
fer functions and context-sensitive parameter binding. Second,
the deep semantics of object-field interactions and cross-
persistence-aware propagation integrates database schema con-
straints and query safety analysis to track multi-script attack script database persistence, leaving a massive attack surface
paths traversing shared database states. Finally, object-aware completely unanalyzed. These deficiencies underscore a fun-
reaching definitions combine calling and bounded variable alias damental question in static vulnerability analysis: how can
contexts to precisely model object-field mutations across method CPG-based taint analysis be made semantic-aware rather
boundaries.
than purely structural, while remaining scalable to large,
We evaluate TaintRadar on both synthetic benchmarks and
real-world applications?
real-worldsystems.OntheSARDbenchmark,TaintRadardras-
tically reduces false positives while maintaining 80% overall In response to this question, we present TaintRadar, a sys-
accuracy. Deployed across 19 real-world PHP applications, it temthattreatstaint-stylevulnerabilitydiscoveryasasequence
rediscovered the majority of known CVEs and uncovered 29 ofindependent,composablegraphaugmentationoperatorsex-
confirmed zero-day vulnerabilities (26 SQL injection and 3
ecutedoveranoptimizedbaseCPGrepresentation.Ratherthan
stored XSS) that have already received CVE identifiers. These
reinventing the underlying parsing architecture, TaintRadar
results demonstrate that semantic-aware graph augmentation
significantly improves the precision, coverage, and practical layers domain-specific security semantics directly onto the
utility of static taint analysis. graphrepresentation.First,targetapplicationcodeisliftedinto
a base CPG that unifies abstract syntax trees (AST), control
Index Terms—static analysis, taint analysis, code property
graphs, SQL injection, XSS, PHP flow graphs (CFG), and program dependence graphs (PDG).
Atopthisfoundation,TaintRadardeploysalanguage-specific
knowledge layer that explicitly maps vulnerability-relevant
I. INTRODUCTION
characteristics—ranging from superglobal request inputs to
Taint-style vulnerabilities represent a critical and highly security-critical execution sinks—grounding the downstream
prevalent class of software security weaknesses in which analysis engine in explicit program semantics rather than
untrusted or malicious input propagates unchecked from un- fragile ad hoc heuristics.
trusted external inputs (sources) to execution-critical opera- Theanalysisthenproceedsthroughthreeindependentgraph
tions(sinks),suchasdynamiccommandexecutionordatabase augmentation stages. A sanitization inference layer com-
interpreters. These vulnerabilities are particularly rampant in putes node-level lattice guarantees for individual vulnerability
PHP-based web applications, which natively process massive classes through targeted forward data-flow reasoning, mak-
volumes of user-controlled inputs through dynamically typed, ing explicit exactly where and how validation logic applies.
string-heavy operations and database-backed workflows. Simultaneously, TaintRadar evaluates persistence-aware taint
Despite significant progress in static analysis frameworks propagation by executing column-specific database schema
leveraging taint tracking and Code Property Graphs (CPG), constraintsandquerysafetyanalyses;thisallowstheengineto
6202
luJ
71
]RC.sc[
1v65461.7062:viXra

<!-- page 2 -->

2
seamlessly track data flows that traverse application-database II. BACKGROUNDONCODEPROPERTYGRAPHS
boundaries across otherwise disconnected scripts. To prevent
Yamaguchi et al. [16] first introduced CPGs to model and
structural noise, an object-aware reaching-definition layer en-
discover vulnerabilities through static code analysis. A CPG
riches the graph by concurrently resolving calling contexts
is a representation of the source code combining multiple
andboundedobject-fieldmutationsacrossmethodboundaries.
classicalprogramanalysisabstractionsintheformofAbstract
Once these semantic layers are woven into the graph, a
Syntax Trees (AST), Control Flow Graphs (CFG), and Pro-
unified backward reachability traversal traces precise paths
gram Dependency Graphs (PDG). As a result of merging the
from unsanitized sinks to untrusted sources, yielding a high-
structure, control flow, and data dependencies of the source
confidence set of actionable vulnerabilities. As an optional
code, researchers and practitioners can frame vulnerability
validation step, TaintRadar cross-references its findings with
analysis as graph traversals on the CPG.
public CVE records to automatically verify coverage against
Given an application’s CPG, most vulnerabilities in the
documented exploits.
sourcecodecanbemodeledasinformationflowproblemsthat
Our comprehensive evaluation demonstrates that Tain-
violate confidentiality or integrity. For example, a Cross-Site
tRadar significantly outperforms state-of-the-art static analy-
Scripting (XSS) attack involves an attacker-controlled input
sistoolsindetectingcomplextaint-stylevulnerabilitieswithin
reaching a sensitive sink displaying it to a console, browser,
real-world PHP applications. On the synthetic SARD [15]
database, or API response. An SQL injection involves a path
benchmark,TaintRadardrasticallycurtailsfalsepositiverates
fromanattacker-controlledsourcetoanSQLqueryexecution,
whilemaintainingarobust80%overalldetectionaccuracyand
allowingunrestrictedescapingormodificationoftheexecuted
an F1-score of 78% on highly complex, structurally diverse
statement. Consequently, Yamaguchi et al. [16] grouped CPG
testcases.Inevaluationsacrosspopularopen-sourcesoftware,
vulnerability descriptions in two broad categories. Syntax-
TaintRadar successfully rediscovered flaws matching 86.2%
only vulnerability descriptions rely solely on the code’s
of known historical CVEs. More importantly, TaintRadar
AST to detect vulnerable code, such as matching all non-
uncovered29previouslyunknownzero-dayvulnerabilitiesthat
constant arguments to the print function. This type of
have since been validated and assigned public CVE IDs,
description fails to capture attacker control or the relationship
including 26 SQL injection and 3 stored cross-site script-
between different statements, ultimately resulting in missed
ing (XSS) vulnerabilities across 6 applications. These results
coverage or multiple false positives. Accordingly, taint-style
demonstratethatsemantic-awaregraphaugmentationprovides
vulnerability descriptions are represented by syntax-only
a scalable, highly precise blueprint for modern static analysis.
descriptions of attacker-controlled sources, security sensitive
This paper makes the following key contributions:
sinks and sanitizers. A path matches a taint-style description
if a data dependency path exists from source to sink without
• Semantic-Aware Sanitization Analysis: We introduce
passing by sanitizer nodes.
a sanitization-aware data-flow framework that computes
Backes et al. [17] laid the foundational work of applying
precise, node-level lattice guarantees using formal trans-
CPG traversal to the high-level dynamic scripting language
fer functions and context-sensitive interprocedural pa-
that is PHP. Their research defined attacker-controlled input
rameter binding, significantly eliminating false positives
and listed different vulnerability classes relevant for PHP
compared to traditional binary taint barriers.
static code analysis. For taint-style vulnerabilities, potential
• Database-Aware Taint Analysis: We systematically in-
sources include all parameters that can be transferred through
tegrate database schema constraints and column-specific
anHTTPrequest,astheyconstituteattacker-controlledinputs.
query safety analyses into static taint analysis. By
PHPwrapsHTTPrequestparametersinvariousassociativear-
drawing targeted persistence-aware dependency edges
rays including: $_GET, $_POST, $_COOKIE, $_REQUEST,
acrosswrite/readboundaries,TaintRadaruncoverscross-
$_SERVER,and$_FILES.Asforthesinknodes,theauthors
script attack vectors—such as SQL injections and stored
include all sensitive functions for each vulnerability, e.g.
XSS—that completely break data flow continuity in con-
mysql_queryforanSQLinjectionattackorechoforXSS.
ventional CPG models.
• Object Dependency Graph Integration: We en-
hance CPG capabilities with Object Dependency Graphs
III. CHALLENGES&RUNNINGEXAMPLES
(ODGs) derived from a context-sensitive reaching defini-
tion analysis. By introducing explicit k-bounded calling We use the running examples in Listings 1, 2, and 3 to
and variable stacks, the engine precisely tracks field mu- motivatethecorelimitationsofstandardCodePropertyGraph
tationsacrossconstructorsandobjectboundarieswithout (CPG) implementations.
risking state-space explosion or infinite loops. Listing 1 shows a classic PHP example of attacker-
• Comprehensive Evaluation and Open-Source Imple- controlled input originating from browser cookies
mentation:Weprovideanextensiveempiricalevaluation ($user_id)andHTTPPOSTrequests($comment_title
across controlled benchmarks and 19 production-grade and $comment). While some variables are sanitized against
applications. We demonstrate decisive performance gains Cross-Site Scripting (XSS)—either explicitly via functional
over SOTA tools and release our complete prototype sanitizers or implicitly through strict type coercion—an
implementation to the research community at https:// unsanitized value directly reaches the SQL statement
anonymous.4open.science/r/TainTRadar-44ED/. construction.

<!-- page 3 -->

3
The values stored in the database are later retrieved by $safe_uid = (int)$uid;
6
the separate script in Listing 2 and rendered to the output 7 $safe_title = htmlentities($title);
$this->sql = "INSERT INTO blog_posts
buffer. The first code sample is vulnerable to SQL Injection 8
VALUES (" . $safe_uid . ", ’" . $
(SQLi) attacks through both the $comment_title and
safe_title . "’, ’" . $comment .
$comment parameters, while the second is vulnerable to a "’)";
Stored (Persistent) XSS attack through the $comment field. }
9
This scenario underscores the need for a principled static10
public function addTenantFilter($tenantId)
analysis tool that computes precise, node-level sanitization11
{
guarantees per vulnerability type and tracks vulnerable data
$this->sql .= " AND tenant_id=" .
12
flows that span persistent storage boundaries such as a (int)$tenantId;
database. }
13
14
TABLE I: Database Schema Mapping public function exec($link) {
15
mysqli_query($link, $this->sql);
16
}
Table Column Type Safe Type 17
}
blog_posts user_id TINYINT True 18
blog_posts comment_title VARCHAR(20) False 19
$dao = new PostDAO($_COOKIE["uid"], $
blog_posts comment VARCHAR(100) False 20
_POST["comment_title"], $
_POST["comment"]);
$dao->addTenantFilter($_GET["t"]);
21
$dao->exec(mysqli_connect("localhost",
Listing 1: Getting user input (input.php) 22
"user", "psswd", "db"));
<?php ?>
1 23
$user_id = (int)$_COOKIE["uid"];
2
3 $comment_title = This work aims to address three fundamental challenges
htmlentities($_POST[’comment_title’]);
exposed by these scenarios:
$comment = $_POST[’comment’];
4 a) Challenge 1: Vulnerability-Typed Sanitization Filter-
5
if (isset($user_id) && isset($comment_title) ing: Existing static analysis tools fail to provide precise,
6
&& isset($comment)) { vulnerability-specific sanitization guarantees, leading to high
7 $link = mysqli_connect("localhost", false-positive rates when sanitization is conditional, partial, or
"user", "psswd", "db");
context-dependent. A security framework must augment the
$sql = "INSERT INTO blog_posts VALUES (" .
8 base CPG with semantic-aware contexts to compute accurate
$user_id . ", ’" . $comment_title .
"’, ’" . $comment . "’)"; sanitization labels at the node level per vulnerability type.
9 mysqli_query($link, $sql); In Listing 1, the $user_id variable is implicitly sanitized
}
10 against injection through strict type coercion to an integer,
?>
11 and $comment_title is properly sanitized against XSS
using htmlentities, yet it remains completely vulnerable
to SQL injection. Conversely, $comment remains entirely
Listing 2: Displaying data (display.php)
un-sanitized. Implicit type conversions can also occur at the
<?php
1 database layout layer where user_id is securely kept as a
$link = mysqli_connect("host", "user",
2
"psswd", "db"); numeric type, rendering any stored XSS payloads ineffective
3 $sql = "SELECT * FROM blog_posts"; within that field.
$result = mysqli_query($link, $sql);
4 The core challenge lies in computing precise, multi-
$rows = mysqli_fetch_all($result,
5 vulnerability sanitization signatures within the graph while
MYSQLI_ASSOC);
dynamically parsing varying language features such as PHP’s
6
foreach ($rows as $row) { loose type juggling.
7
8 echo "User: " . $row["user_id"] . " - b) Challenge 2: Database-Aware Cross-Boundary Vul-
Comment: " . $row["comment_title"] .
nerability Detection: Traditional static analysis tools eval-
":\n" . $row["comment"];
uate application code in isolation from database structures,
}
9
?> completely missing vulnerability paths that traverse persistent
10
storage boundaries. In our running example, the taint chain
initiates during the database insertion step in input.php
Listing 3: PHP Object-oriented running example (Listing 1) and completes via unsafe retrieval and rendering
(PostDAO.php) inside display.php (Listing 2), despite the lack of direct
1 <?php static code connections (such as file inclusions or shared
class PostDAO {
2 function calls) between the scripts.
public $sql;
3 Resolving this requires parsing database configurations to
4
public function __construct($uid, $title, $ verify column constraints (per Table I), auditing query struc-
5
comment) { tural safety, and mapping attack vectors across persistence

<!-- page 4 -->

4
barriers. The core challenge involves modeling how tainted identifyzero-days.Asdetailedin§V,thismodulararchitecture
parameters committed to storage in one request lifecycle significantly improves detection coverage and precision while
propagate into separate execution scopes, particularly under scaling efficiently to large, real-world production codebases.
differing sanitization requirements for data storage versus
subsequent output generation.
c) Challenge 3: High-Fidelity Tracking of Object-Field B. Preliminaries and Definitions
Data Flows: Standard CPG representations often fail to cap-
To ease our explanation of TaintRadar design details, we
ture fine-grained data dependencies for individual object field
first introduce preliminaries and definitions.
attributes. This introduces severe visibility gaps in modern
Let G=(V,E,λ,µ) be a Code Property Graph, where
object-oriented codebases where attacker-controlled variables
mutate shared internal object states across method and con- V : set of AST nodes
structor boundaries.
E =E ∪E ∪E : set of AST, CFG, and PDG edges
AST CFG PDG
In Listing 3, external user values are combined into
λ:E →L : edge labeling function
the class property $this->sql inside the constructor,
µ:(V ∪E)×K →S : node and edge property function
conditionally appended via a secondary method invocation
(addTenantFilter()), and ultimately executed inside
The property function µ(v,k) is simplified as v .
k
exec(). Although $user_id undergoes implicit integer
Definition 1 (MATCH Traversal): Following the original
castingand$comment_titleissafelyfilteredagainstXSS,
CPGframework[16],wedefinethereusableMATCHtraversal
the un-sanitized string $comment flows directly into the
for node filtering:
database engine via object field manipulation.
The core challenge lies in extending the CPG infrastructure
MATCH (V)=FILTER ◦TNODES(V)
p p
with object-field dependency tracking that models field read-
/write assignments, reference aliasing, interprocedural param- where
eter bindings, and class framework patterns while preserving
TNODES(V): traverses from roots in V to all AST nodes
rigorous context-sensitivity.
FILTER : filters nodes according to predicate p
p
IV. APPROACH
p:V →{true,false}: node property predicate
A. TaintRadar Design Overview
Definition 2 (Vulnerability Configuration): A vulnerability
TaintRadar is built on a central guiding principle: vulner-
is characterized by its sanitization and sink functions
ability discovery can be reduced to a reachability problem
over a semantically enriched CPG. Instead of introducing a V =(Φ,Σ)
new analysis formalism, TaintRadar incrementally augments
a base CPG with critical security-relevant semantic relations. where
Φ⊆F is the set of sanitization functions
As illustrated in Figure 1, the framework ingests source code
to construct a base graph, then sequentially applies three Σ⊆F is the set of sink functions
independent, composable augmentation operators designed to F is the universe of all functions.
systematically bridge historical precision and recall gaps:
Intuitively, Ω captures sources such as $_GET or $_POST
id
• Sanitization Augmentation computes vulnerability- in PHP.
specific, node-level safety guarantees via context-
Definition 3 (Global Attack Sources): Attack sources are
sensitive parameter binding, avoiding the coarse binary
vulnerability-independent and capture (i) syntactic identifiers
barrier modeling of prior tools.
that directly reference attacker-controlled inputs and (ii) API
• Database Integration establishes persistence-aware de-
calls that return attacker-controlled values. They are defined
pendencies by leveraging schema constraints and query
as
safetyproperties,explicitlylinkingrelatedwriteandread
operations across distinct scripts. Ω=Ω id ∪Ω func ,
• Object Dependency Augmentation injects precise
where Ω ⊆ S is the set of untrusted source identifiers, and
object-field dependencies derived from context-sensitive id
Ω ⊆F is the set of untrusted source functions.
reaching definitions to reliably track field mutations func
Definition 4 (Database Schema): Let D = (T,C,τ) be a
across method boundaries.
database schema where
Followingtheseaugmentations,TaintRadarexecutesauni-
fiedBackwardTraversalovertheenricheddependencygraph T : set of table names
to discover source-to-sink paths where an attacker-controlled C : set of column names
source can reach a sensitive sink without a sufficient sanitiza- τ :T ×C →{0,1} : column safety function
tion guarantee. The resulting candidate exploit paths undergo
automated Exploit Validation by cross-referencing public where τ(t,c) = 1 indicates column c in table t has a safe
CVE databases to flag known vulnerabilities. Unmatched type such as INT or DATETIME and τ(t,c)=0 indicates an
findings are then manually audited to verify exploitability and unsafe type such as VARCHAR or TEXT.

<!-- page 5 -->

5
Language-Specific
Configuration
Sin S k I a n F n p u it u n iz t c a S ti t o o i o n u n s r c F p e u e F n r u c V n t u i c o ln t n io e s r n p a s b e r i l i t y A S u a g n m it e i z n a t t a i t o io n n C D S o a c n t h a s e b t r m a a s i a n e t Obje I c n t t e D g e r p a e ti n o d n e ncy B Tr a a c v k e w r a s r a d l Dis S c i o n v k e ry P C u V b E li s c
Vulnerability Integration
CPG Enhanced CPG
CPG P arsing
Application List of
Source vulnerable Exploit Exploitable
Code paths Validation Vulnerabilities
Fig. 1: TaintRadar system overview.
C. TaintRadar Design Details Literal values are considered sanitized by default:
Language-Specific Configuration. While TaintRadar’s MATCH (v)⇒TV(v,_,_)=1 (1)
literal
analysispipelineisgeneral,itspracticaleffectivenessdepends
on a language-specific configuration. For PHP, this config- Field identifiers are handled by checking against known
uration defines attacker-controlled sources (e.g., superglobal tainted identifiers, global constants (such as magic constants
arrays such as $_GET and $_POST), built-in and library in PHP), and a pre-built constant table (a mapping between
sanitization functions, and security-sensitive sinks such as defined constants and their values):
mysql_query. This PHP-specific schema reflects the lan-
MATCH (v)⇒
field
guage’s security-critical constructs and guides TaintRadar’s

0 v ∈Ω
tain
T
t
he
pro
v
p
u
a
ln
g
e
a
r
ti
a
o
b
n
ili
a
ti
n
e
d
s
v
c
u
o
l
v
n
e
e
r
r
e
a
d
bil
b
it
y
y
T
d
a
et
i
e
n
c
tR
tio
a
n
d
.
ar include SQL TV(v,D,πV)=
1
v
n
n
a
a
m
m
e
e ∈Ξ m
id
agic
i
C
n
r
j
o
ec
ss
ti
-
o
S
n
it
,
e
c
S
o
c
m
ri
m
pt
a
in
n
g
d
(
e
X
x
S
ec
S
u
)
t
,
i
a
o
r
n
b
,
it
c
ra
o
r
d
y
e
fi
i
l
n
e
je
r
c
e
t
a
i
d
o
s
n
/
,
w
fi
ri
l
t
e
es
i
,
n
a
c
n
lu
d
s
s
io
e
n
s-
, (cid:86) SV(Co
d
nstTable[v name ])
o
v n
th
am
e
e
rw
∈
is
C
e
onstTable
sionfixationattacks.Thisspecificationphasedirectlysupports d∈D
our sanitization and sink analysis, grounding the rest of the For identifiers, we traverse their reaching definition edges
pipeline in precise, context-aware logic. to check for prior sanitization or type casting to a safe type.
SanitizationAugmentation.TaintRadarreliesonourSan- Additionally, we check whether the identifier is passed by
itization Analysis algorithm: a forward data flow analysis reference to a function call:
that processes the CPG to compute sanitization guarantees
MATCH (v)⇒
identifier
per vulnerability type, effectively addressing Challenge 1 in

0 v ∈Ω
Sec
D
ti
e
o
fi
n
ni
I
t
I
io
I.
n
W
5
e
(
d
S
e
a
fi
n
n
it
e
iz
o
a
u
ti
r
on
do
D
m
o
a
m
in
ai
a
n
n
)
d
: T
fu
h
n
e
ct
a
io
n
n
aly
b
s
e
i
l
s
ow
o
.
perates
1
v
name
∈Σ
id
TV(v,D,πV)= type safe
over the tw
0
o-p
:
oin
u
t
n
l
s
a
a
tt
n
ic
it
e
iz
(
e
{
d
0,
(p
1
o
}
t
,
e
≤
nt
)
i
,
al
w
ly
he
t
r
a
e
inted)
 T
(cid:86)
b V
d
yr
∈
e
D
f (v
d
,πV) v
oth
p
e
a
r
s
w
se
i
d
se
by reference
1 : sanitized (guaranteed safe)
For identifiers passed by reference, we follow their corre-
0≤1 : “sanitized” is better than “unsanitized”
sponding parameter within the function body:
a) Sanitization Function:: For vulnerability type V = TV (v,πV)=
byref
(Φ,Σ), we compute the node-level sanitization function (cid:40)
SV(ReturnDef(f,param_idx(v))) if ∃f :v ∈ByRefArgs(f)
SV(v)=TV(cid:0) v,{SV(u):u∈PRODUCERS(v)},πV(cid:1)
, (cid:86) SV(d) otherwise
d∈PRODUCERS(v)
where (2)
PRODUCERS(v) ={u∈V :(u,v)∈E∧λ((u,v))=D}, where ByRefArgs(f) denotes arguments passed by refer-
data-dependency predecessors of v, ence to function f, param_idx(v) returns the parameter index
TV : V ×P({0,1})×Π→{0,1}, of argument v, and ReturnDef(f,i) returns the return-point
transfer function, definition of the ith parameter of function f.
Function calls fall into two categories: user-defined or un-
πV ∈ Π, parameter context for
defined. For calls to undefined methods (no call body present
interprocedural analysis.
in the application code), we apply the following heuristics:
The sanitization function applies different logic based on known sanitization functions and functions that return safe
the node type. We define the transfer function TV for each types (e.g., hashing, boolean operators, casting functions)
node category as follows. are treated as sanitized, otherwise functions are considered

<!-- page 6 -->

6
sanitizedonlyifalltheirargumentsaresanitized.Thisincludes <?php
1
built-in operators like =, +, and *, which are function calls 2 $name = $_GET[’name’];
echo("Hello ". htmlentities($name));
with syntactic sugar. For user-defined methods, we implement 3
?>
an interprocedural static analysis. 4
MATCH (v)⇒ This sanitization layer enables TaintRadar to tag nodes as
call
 either clean or tainted. It directly addresses Challenge 1,
 1
1
v
v
n
ty
a
p
m
e
e
∈
∈
Σ
Φ
safe
e
th
n
i
s
s
ur
c
in
o
g
nte
t
x
h
t
at
is
sa
p
n
r
i
e
ti
s
z
e
a
r
t
v
i
e
o
d
n
t
i
h
s
ro
c
u
o
g
r
h
re
o
c
u
t
t
ly
th
i
e
nte
d
r
a
p
t
r
a
ete
fl
d
ow
an
o
d
f
th
th
a
e
t
TV(v,D,πV)= 0 v ∈Ω code. These tags are propagated and used to add further
name func
 T
(cid:86)
in
V
terpro
d
c
(v,πV) i
o
f
th
u
e
s
r
e
w
r-
i
d
s
e
e
fined function c
w
o
h
n
e
s
r
t
e
ra
d
in
e
t
e
s
p
t
e
o
r
t
i
h
n
e
si
d
g
a
h
t
t
a
s
fl
in
o
t
w
o
s
b
t
o
h
t
a
h
t
d
re
ir
a
e
c
c
h
t
d
an
at
d
ab
in
a
d
se
ire
c
c
o
t
m
d
p
a
u
ta
tat
fl
io
o
n
w
s
s
,
d∈D are required. This includes the use of query and table-level
(3)
constraintstoimprovetheprecisionofvulnerabilitydetection.
b) Interprocedural Analysis:: For user-defined function
calls, we perform context-sensitive analysis with parameter
binding.
TV (v,πV)=SV(ReturnBlock(Callee(v)))
interproc
where the parameter context is defined as:
πV =[SV(ARG1),SV(ARG2),...,SV(ARGn)]
v v v v
c) Context Binding:: For a function call v with callee f
and arguments {ARGi}n , the parameter context πV binds
v i=1 v
each formal parameter p ∈ Params(f) to the sanitization
i Fig. 3: Sanitized code’s augmented AST
status of its corresponding actual argument:
Database Integration. The main challenge with establish-
SV(p )=SV(ARGi) for i=1,...,n
i v ing a sound relationship between code and data is that scripts
This context is propagated throughout the analysis of func- sharing persistent storage can appear as functionally separate
tion f’s body, ensuring that parameter sanitization status within the CPG. For example, in the PHP code of Listing
reflects the calling context. 1 and 2, data from input.php can easily reach display.php:
d) Example Walkthrough:: As illustration, we apply the independentcodesnippetscansharedatathroughthedatabase
sanitization algorithm above to a simple PHP code that without explicit data dependency (reaching definition) edges.
retrieves the name parameter from an HTTP request and To address this limitation, TaintRadar treats persistence as
displays it back: a simple XSS vulnerability. a first-class part of taint propagation. It parses the database
schema and the SQL statements in code to (i) classify queries
<?php
1 asSAFE/UNSAFE underschema-andsanitization-awarerules
$name = $_GET[’name’];
2
echo("Hello ". $name); and (ii) introduce database dependency edges that reconnect
3
?> write operations (INSERT/UPDATE) to read operations (SE-
4
LECT) when both sides carry unsafe data. This augmentation
Figure2showstheaugmentedASTproduced:sanitizednodes
targets Challenge 2 by enabling cross-script vulnerability
are colored in green and unsanitized ones red. As expected,
discovery that is impossible when the database is modeled
the sink function (echo) is unsanitized.
as a black box.
Since SQL statements in applications interleave con-
stants, variables, and function calls, TaintRadar performs a
lightweight query parsing step to extract: (1) the target table,
(2) the referenced columns, and (3) the CPG value nodes that
supplyuser-controlledinputs.Concretely,theparsertokenizes
query strings, normalizes common syntactic variations (e.g.,
whitespace, quoting, concatenation), and applies pattern- and
clause-aware extraction rules for SELECT/INSERT/UPDATE
forms to recover (t ,C ,V ). The extracted metadata is then
q q q
Fig. 2: Unsanitized code’s augmented AST attached to the corresponding SQL call nodes in the CPG and
used to drive the safety and dependency rules below.
In contrast, the following code sanitizes user input before Definition 6 (Query Parsing): For an SQL query q, we
displaying it. Figure 3 shows how the htmlentities function define
sanitizes the sink function, preventing the XSS vulnerability. PARSE(q)=(t ,C ,V )
q q q

<!-- page 7 -->

7
where SQL injection attacks the normal execution of a query is
t : target table (t ∈T)
q q escaped to execute arbitrary commands on the database itself.
C : affected columns (C ⊆C) Therefore, database-aware taint analysis isn’t applied when
q q
detecting paths vulnerable to SQL injection.
V : CPG value nodes (V ⊆V)
q q
f) Example Walkthrough:: The SELECT statement in
Notethattheabovedefinitionimpliesreturningallcolumns
Listing 2 will be labeled as unsafe because one of its relevant
C of a table t if the wildcard symbol (*) is used.
columns (comments) is of an unsafe type as specified by
e) QuerySafetyAnalysis:: WeanalyzethesafetyofSQL
its database schema. In Listing 1, the INSERT statement
statements extracted from the application code as follows.
will also be labeled as unsafe since the value inserted into
SELECT queries q are safe if their associated columns C
q the comments column isn’t sanitized before it was added to
are of safe type or passed to safe SQL functions: the query command ($comment). This concludes the CPG
augmentation part in TaintRadar as illustrated in Figure 1.
(cid:40)
QSELECT(q)=
SAFE if∀c∈Cq:τ(tq,c)=1∨c∈ΦSQL Compared to the SOTA, these new enhancements enrich the
CPG, ensuring backward traversal can uncover vulnerabilities
UNSAFE otherwise
that span both code and persistent state.
where Φ SQL = {COUNT,SUM,LENGTH,...} are safe Object-aware Data Flow Analysis. A key limitation of
SQL built-in functions. many CPG-based taint analyses on object-oriented code is
INSERT or UPDATE queries q are safe if the values the lack of explicit, field-level dependencies across method
inserted into unsafe columns are properly sanitized: boundaries.Inpractice,security-relevantvaluesarefrequently
storedinobjectfields,initializedinconstructors,andmodified
(cid:40)
in helper methods (including static utilities and dynamically
QINSERT(q)= S
(cid:86)
AFE
SV(v)
i
o
f
t
{
h
c
e
∈
r
C
w
q
i
:τ
se
(tq,c)=0}=∅
dispatched calls). When these field updates are not repre-
v∈Vunsafe sented as dependencies, taint propagation either breaks (false
where V = {v ∈ V : c ∈ C ∧τ(t ,c ) = 0} and negatives) or becomes overly conservative (false positives).
unsafe i q i q q i
SV(v) is the sanitization status from the previous analysis. To address this gap (Challenge 3), TaintRadar augments the
This ensures that tainted data doesn’t leak into persistent CPGwithobjectdependencyrelationsderivedfromacontext-
storage without being sanitized first. sensitive reaching-definitions analysis that tracks field writes
Definition 7 (Database Dependency Edge): We add a and resolves object aliasing across call boundaries.
databasedependencyedgefromanINSERTorUPDATEquery Definition 8 (Object-aware Reaching Definition Domain):
q to a SELECT query q if: The analysis operates over the domain of definition sets with
1 2
calling context:
1. PARSE(q )=(t ,C ,V ) and PARSE(q )=(t ,C ,V )
1 1 1 1 2 2 2 2
2. t =t (same table) D =P(V ×Context)
1 2
3. C 1 ∩C 2 ̸=∅ (overlapping columns)1 where
4. QINSERT(q 1 )=UNSAFE V : CPG node
and QSELECT(q )=UNSAFE
2 Context : CallStack×VarStack
CallStack : callstack[c ,c ,...,c ]withc ∈V caller
This creates additional data dependency edges E in the 1 2 n i
DB nodes in reverse chronological order
augmented CPG: G′ =(V,E∪E ,λ,µ), enabling taint flow
DB VarStack : variable stack [v ,v ,...,v ] with v rep-
1 2 n i
detectionacrossdatabaseboundariesevenwhennodirectcode
resentingvariablenamesofthesameobject
dependenciesexistbetweenseparatescripts.Thebenefitofthis
across different scopes
type of graph augmentation is two-fold:
1) Discovery of further vulnerable paths: The database Thecallandvariablestacksareusedtostoreprocedurecalls
dependency edges allow us to traverse sink functions andvariablenames.Whentheanalysisreachesafunctioncall,
across the database beyond the traditional CPG. the caller’s node is added to the call stack before switching to
2) Reduction of false positives: Incorporating the con- the callee’s body. The variable name stack follows the same
straints imposed by the database schema will rule out logic for handling different parameter names referring to the
safe queries, decreasing the number of false positives. same object or switching to the reserved keyword this for
constructors and member functions.
Database-aware taint analysis can be applied to all types
of vulnerabilities except for SQL injection. While its most For node v and variable name x, we compute the context-
popular form is persistent XSS, it is just as feasible for other sensitive reaching definition function:
vulnerability classes. For example, an input source can be
saved to the database before being executed by the operating RO(v,x,κ)=TO(cid:0) v,x,{RO(u,x,κ′):u∈PRODUCERS(v)},κ (cid:1)
system: a stored form of command injection. However, for
where TO is the object-aware transfer function and κ′
1A limitationof this definitionis that itmisses the edgecase where only
safecolumnsoverlap represents the appropriate context for each predecessor.

<!-- page 8 -->

8
The transfer function TO : V × String × P(P(V × Algorithm 1 Backward Traversal
Context))×Context→P(V×Context)processesnodesbased 1: procedureBACKWARDTRAVERSAL(cpg,endNode,startNodes)
on their types: 2: functionBACKWARDTRAVERSE(paths,startNodes)
3: output←[]
Assignment nodes create new definitions: 4: forallpathinpathsdo
5: lastNode←path.last
MATCH (v,x)⇒TO(v,x,_,κ)={(v,κ)} (4) 6: iflastNodeisMethodParameterthen
assignment 7: reachingDefs ←
Method.Callees.Arguments[lastNode.index]
Field access operations require object-aware analysis
8: else
across method boundaries: 9: reachingDefs←lastNode.reachingDefinitions
10: endif
MATCH (v,x)⇒ 11: ifreachingDefs.emptythen
fieldAccess 12: output.append(path)
TO(v,x,D,κ)=ComputeFieldDefs(v,x,κ) (5) 13: endif
14: foralldef inreachingDefsdo
15: ifdef notinpaththen
Here, ComputeFieldDefs resolves definitions across object 16: output.append(path+def)
instances and method boundaries. 17: endif
18: endfor
Constructor and method calls require context-sensitive 19: endfor
inter-procedural analysis: 20: ifpaths=outputthen
21: return[]
MATCH (v,x)⇒ 22: elseif∃path∈pathssuchthatstartNodes∈paththen
call 23: returnpathcontainingstartNodes
TO(v,x,D,κ)=RO(ReturnBlock(Callee(v)),x′,κ′) (6) 24: else
25: returnBACKWARDTRAVERSE(output,startNodes)
26: endif
Here x′ is the parameter name in the callee and κ′ is the 27: endfunction
updated calling context. 28: path←BACKWARDTRAVERSE([[endNode]],startNodes)
29: endprocedure
Method parameters connect to their corresponding argu-
ments at call sites:
Algorithm 2 Vulnerable Sink Discovery
MATCH (v,x)⇒
parameter 1: procedureGETVULNERABLEPATHS(G,V=(Φ,Σ))
TO(v,x,D,κ)=
(cid:91)
RO(Argument i (c),x,κ c ) (7)
2
3
:
:
s
s
o
in
u
k
r
s
ce
←
s←
FI
F
N
I
D
N
_
D
N
_
O
N
D
O
E
D
S
E
(G
S(
,
G
Σ
,
)
Ωid∪Ωfunc)
4: sinks←sinks.filter(λs:SV(s)=0) ▷Onlyunsanitizedsinks
c∈CallSites(v)
5: vulnerablePaths←∅
where Argument (c) is the ith argument of call c and κ its 6: forallsink∈sinksdo
i c 7: paths←BackwardTraversal(G,sink,sources)
calling context. 8: vulnerablePaths←vulnerablePaths∪paths
9: endfor
Control flow boundaries handle interprocedural returns:
10: P ←DEDUPLICATE_BY_ENDPOINTS(vulnerablePaths)
11: returnP
PRODUCERS(v)=∅ (8) 12: endprocedure
(cid:40)
∅ if CallStack(κ)=∅
⇒TO(v,x,D,κ)=
RO(CallSite(κ),x,PopContext(κ))
indicates that the reaching definitions for the current traversal
Regular data flow propagates existing definitions: have been exhausted without reaching any start nodes, so
(cid:91) the algorithm can safely terminate with an empty path. This
otherwise⇒TO(v,x,D,κ)= {(d,κ )} (9)
d traversal mechanism forms the core engine for identifying
(d,κd)∈D candidate vulnerable paths.
Sanitization and database augmentations ensure that our Vulnerable Sink Discovery and CVE Matching. We
traversal accounts for inter-procedural object behaviors. now systematically identify vulnerable paths. As detailed in
Backward Traversal. With sanitization, database, and ob- Algorithm2,wedefinevulnerablesinksassensitivefunctions
ject links established, we now address the need for robust that can be reached by unsanitized, attacker-controlled data.
data flow analysis: tracing tainted sinks back to their sources. Combining our previous modules, we automate the discovery
Priorworkssuchas[16],[17]introducedbackwardtraversalto of these paths across the codebase. Source and sink nodes
detect vulnerable paths. Our main contribution in TaintRadar are language-specific configurations as defined in Definitions
istostrengthenthismechanismthroughtheintegrationofdata 3 and 2 respectively.
dependency walks in object-oriented contexts. Asafinalstep,TaintRadariteratesoverreportedvulnerable
TaintRadar’sbackwardtraversal(Algorithm1)isabreadth- paths and cross-references them with public CVEs. Matching
first and context-aware search: it expands the end node (sink leverages NLP techniques including tokenization, stopword
function) through data dependency edges (as defined previ- filtering, and regex-based pattern extraction to identify key
ously)untilitreachesstartnodes(attacker-controlledsources). filtering criteria such as file names, vulnerable functions,
It has a recursive implementation that recursively expands application versions, and code parameters from unstructured
the paths variable by adding all reaching definitions of each CVEdescriptions.Weemploymulti-layeredsemanticanalysis
path’s last node until it reaches a node contained in the list with vulnerability type classification, version comparison, and
startNodes, in which case the relevant path is returned. Re- parameter-based code matching using substring identification
peatednodesaren’tappendedtothepathtopreventcyclesand with word boundary detection. This best-effort approach al-
infiniteloops.Ifpathsremainsunchangedafteraniteration,it lows us to filter reported vulnerable paths into those asso-

<!-- page 9 -->

9
ciated with a publicly available exploit. This step confirms controlled environments. These tests include both pos-
exploitability, addressing the final challenge and ensuring the itive (vulnerable) and negative (secure) cases, allowing
precision and relevance of our analysis results. us to assess detection accuracy and false positive rates.
2) Evaluation Dataset: We leverage SARD, a large-scale
V. EVALUATION benchmark containing real-world and synthetic vulnera-
We evaluate TaintRadar across synthetic unit tests, bench- bility samples mapped to Common Weakness Enumera-
markdatasets,andreal-worldPHPapplications.Thefollowing tion (CWE) categories (Table III in the Appendix). This
questions guide our evaluation: enablessystematicbenchmarkingofTaintRadaragainst
controlledvulnerabilitiesacrosssimplecodesnippetsfor
1) RQ1 (Effectiveness): How accurately does TaintRadar
PHP.
detect taint-style vulnerabilities on labeled benchmark
3) Full Application Code: TaintRadar is used on com-
datasetscomparedtostate-of-the-artstaticanalysistools
plete open-source and enterprise applications to assess
and learning-based approaches?
its performance on real-world codebases. At this stage,
2) RQ2 (Real-World Applicability): How effective is
we measure scalability and accuracy in identifying vul-
TaintRadaratdetectingknownandpreviouslyunknown
nerabilities within large and complex applications, en-
vulnerabilities in real-world PHP applications?
suringthatTaintRadarcanbeleveragedbypractitioners
3) RQ3 (Precision and Practicality): Does TaintRadar
and researchers.
reducefalsepositivesandredundantreportscomparedto
existingtoolswhenrediscoveringknownvulnerabilities?
4) RQ4 (Component Contribution): How do individ- B. RQ1: Effectiveness on Benchmarks
ual components of TaintRadar’s analysis pipeline con-
WefirstevaluateTaintRadaronhandcraftedPHPunittests
tribute to detection coverage and precision?
designed to exercise complex taint flows, including pass-by-
To answer RQ1, we evaluate TaintRadar on handcrafted
referencepropagation,inter-proceduralflows,object-awarede-
unittestsandtheSARDbenchmark[15],comparingitagainst
pendencies,andfileinclusion.TaintRadarcorrectlyclassified
established static analyzers (Pixy [8], Kave [1], and PHPCor-
all 48 test cases as vulnerable or secure.
rector [18]) using standard classification metrics. To address
We further evaluate TaintRadar on the SARD benchmark
RQ2 and RQ3, we evaluate TaintRadar on large-scale, real-
for PHP, comparing it with KAVe [1], WAP [20], Pixy [8],
world PHP applications in with respect to publicly reported
and PHPCorrector [18]. As shown in Table VI, TaintRadar
CVEs and the state-of-the-art (FIXX [19]). Finally, to answer
achieves the highest overall accuracy (80.14%), and F1 score
RQ4,weconductadetailedablationstudyisolatingtheimpact
(77.65%), outperforming other benchmarks by at least 14%
of each major component in TaintRadar’s pipeline.
while maintaining a low false positive rate (7.12%).
A. Dataset and Experimental Setup Takeaway1:TaintRadarsubstantiallyoutperformsexistingstatic
analyzers on labeled benchmarks, demonstrating that semantic-
Dataset. Our evaluation covers diverse codebase complex-
aware taint propagation and sanitization modeling significantly
ities and application contexts. We extract 1,000 PHP snippets
improve detection accuracy without inflating false positives.
from the SARD dataset [15], each labeled as exploitable or
non-exploitable, and select 19 popular GitHub-hosted PHP
applicationsrangingfromsimplewebtoolstomaturesystems. C. RQ2: Effectiveness on Real-World Applications: CVEs +
This benchmark enables a rigorous, fair comparison against Zero-Days
state-of-the-art tools like FIXX [19]. Targets and baselines
WeevaluateTaintRadaron19real-worldPHPapplications
were selected based on three criteria: (i) public availability
of varying size and complexity. For each application, we
and reproducibility, (ii) representative coverage of CPG/taint
compare detected vulnerable paths against publicly disclosed
static analyzers evaluated in prior work, and (iii) the presence
CVEsandmanuallyvalidatenewlydiscoveredissues.TableII
of ground-truth labels (SARD) or verifiable CVEs.
summarizes the results.
Environment. The evaluation environment consists of an
Across all applications, TaintRadar successfully re-
Ubuntu20.04LTSmachinewith10cores(3.1GHzeach)and
discovered all but six known CVEs and uncovered 29 con-
32GBofRAM.TaintRadarisusedtoparsetheapplication’s
firmedzero-dayvulnerabilities(26SQLinjectionsand3XSS)
code into a CPG, augment it with relevant sanitization and
across 6 distinct applications.
database query labeling, and perform vulnerable sink discov-
ery through backward traversal. Python scripts are used to
Takeaway 2: TaintRadar demonstrates strong real-world effec-
cross-reference the vulnerable paths produced with the CVE
tiveness by reliably rediscovering known CVEs and uncovering
database.Wealsomanuallyverifyasmallsampleofthepaths previously unknown vulnerabilities across diverse PHP applica-
to group them into zero-days or false positives. tions,confirmingitspracticalutilitybeyondbenchmarkdatasets.
Evaluation Stages. TaintRadar is evaluated at 3 different
levels:
D. RQ3: Precision and Vulnerability Report Quality
1) Handwritten Unit Tests: We design targeted unit tests
capturing complex coding logic to test whether Tain- We further evaluate TaintRadar on 19 real-world PHP
tRadarisabletocapturecomplextaintflowinisolated, applica- tions of different size. As shown in Table IV, we

<!-- page 10 -->

10
Exp.Paths Re-Discover Exp.Paths Re-Discover
ApplicationName LOC OriginalCVE FIXX CVEFIXX TaintRadar CVETaintRadar #Zero-Days
SeoPanel02 255.0k CVE-2021-3002(XSS) 2 No 13 Yes -
Collabtive98 180.4k CVE-2021-3298(XSS) 9 No 0 No -
CVE-2024-46240(XSS) 9 Yes 0 No
CVE-2024-48706(XSS) 9 Yes 0 No
CVE-2024-48707(XSS) 9 Yes 0 No
CVE-2024-48708(XSS) 9 Yes 0 No
osCommerceCEPhoenix58 59.8k CVE-2020-12058(XSS) 0 No 22 Yes -
ClansphereCMS110 54.3k CVE-2021-27310(XSS) 20 No 3 Yes -
FantasticBlog12 24.7k CVE-2022-28512(SQL) 9 Yes 1 Yes
FantasticBlog31 24.7k CVE-2021-26231(SQL) 42 Yes 1 Yes 1AccXSSCVE-2025-65337
FantasticBlog24 24.7k CVE-2021-26224(XSS) 0 No 1 Yes
EngineersOnlinePortal83 15.6k CVE-2023-5283(SQL) 258 Yes 69 Yes -
EngineersOnlinePortal76 15.6k CVE-2023-5276(SQL) 52 Yes 73 Yes
HospitalManagement CVE-2021-39411(XSS) 6 Yes 6 Yes
System11 9.4k CVE-2024-46237(XSS) 10 Yes 11 Yes
6AccSQLCVE-2025-65340,CVE-2025-69942-45&49
CVE-2024-46238(XSS) 9 Yes 5 Yes
CVE-2024-46239(XSS) 9 Yes 15 Yes
AdvocateOffice
ManagementSystem28 9k CVE-2024-9328(SQL) 21 Yes 24 Yes –
AutomatedEnrollment94 7.7k CVE-2021-3294(XSS) 21 Yes 15 Yes
6AccCVE-2025-67403-08SQL
AutomatedEnrollment26 7.7k CVE-2021-26226(SQL) 0 No 17 Yes
TailorManagement60 7.7k CVE-2021-40260(XSS) 86 Yes 11 Yes
2ACCSQLICVECVE-2025-69941&4716(SQLI)
TailorManagement73 7.7k CVE-2020-36073(SQL) 130 Yes 7 Yes
FruitsBazar89 6.4k CVE-2022-34989(SQL) 4 Yes 8 Yes 1(SQL)AccCVE-2025-65336
FruitsBazar78 6.4k CVE-2022-30478(SQL) 1 Yes 14 Yes 1(XSS)AccCVE-2025-65341
BloodBankSystem27 4.8k CVE-2024-9327(SQL) 5 Yes 23 Yes
1Acc(XSS)CVE-2025-65342
BloodBankSystem04 4.8k CVE-2024-9804(SQL) 84 Yes 22 Yes
Covid19tms04 4.1k CVE-2024-53604(SQL) 42 Yes 5 Yes
-
Covid19tms03 4.1k CVE-2024-53603(SQL) 43 Yes 11 Yes
DairyFarmManagement93 3k CVE-2023-41593(XSS) 33 Yes 12 Yes
CVE-2024-46241(XSS) 9 Yes 14 Yes -
CodeAstro68 2.6k CVE-2024-25868(XSS) 38 Yes 2 Yes
CVE-2024-46236(XSS) 5 Yes 3 Yes
CVE-2024-48709(XSS) 9 Yes 2 Yes 8SQLAccCVE-2025-69930-38
CodeAstro72 2.6k CVE-2024-46472(SQL) 44 Yes 3 Yes
CodeAstro33 2.6k CVE-2024-2333(SQL) 17 Yes 1 Yes
LoanManagement90 2.2k CVE-2024-0900(SQL) 40 Yes 15 Yes 2ACCSQLCVE-2025-69946&48
DailyExpenseTracker06 1.7k CVE-2020-10106(SQL) 10 Yes 4 Yes -
DailyExpenseTracker04 1.7k CVE-2021-26304(XSS) 27 Yes 6 Yes -
VisitorManagement83 1k CVE-2024-22983(SQL) 21 Yes 2 Yes -
VisitorManagement60 1k CVE-2020-25760(SQL) 0 Yes 0 No -
VisitorManagement61 1k CVE-2020-25761(XSS) 21 Yes 4 Yes -
Keerti00 968 CVE-2024-1700(XSS) 11 Yes 5 No -
Total - - 992 35(Yes)6(No) 361 35(Yes)7(No) Fixx[19]→0ZeroDays-Ours→29ZeroDays
TABLE II: Comparison with FIXX [19] on 19 PHP web applications + number of zero days (# Zero-Days) discovered by
TaintRadar.
CWE Description E. RQ4: Contribution of Analysis Components
PHP test SyntheticPHPtestcasesfocusingonSQLinjection We conduct an ablation study isolating the impact of indi-
suite (SQLi)andCross-siteScriptingXSS vidualcomponentsofTaintRadar,asshowninTableV.Start-
ing from vanilla Joern, we incrementally add inter-procedural
TABLE III: Targeted CWE Vulnerabilities for PHP
dataflow, sanitization analysis, and database-aware taint prop-
agation. Inter-procedural and object-aware dataflow substan-
tially increase detection coverage. Sanitization modeling cuts
detect multiple vulnerable paths among the 7 vulnerability reported paths by up to half, directly lowering false positives.
typesconsidered,demonstratingsuperiorcoverageacrossPHP Finally, database-aware augmentation enables detection of
and vulnerability classes. vulnerabilities spanning persistent storage, particularly stored
We then compare TaintRadar against FIXX [19], the state- XSS.
of-the-art PHP vulnerability detection tool, on the same set of
real-world applications. While both tools rediscover a similar Takeaway 4: Each component of TaintRadar’s pipeline con-
tributes meaningfully to either detection coverage or precision,
setofCVEs,TaintRadarconsistentlyreportsfewervulnerable
validating the design choice of combining semantic dataflow,
paths for the same vulnerability. When both systems re-
sanitization reasoning, and database-aware analysis.
discover the same CVE, TaintRadar usually returns fewer
vulnerable paths, demonstrating better pruning and higher
precision. For example, in Clansphere, FIXX reports 20 re- VI. RELATEDWORK
flected XSS paths, while TaintRadar reports only 3; in Tailor We position TaintRadar with respect to previous works
Management,FIXXreports130SQLinjectionpathscompared on taint-style vulnerability detection through CPG traversals.
to7reportedbyTaintRadar;andinLoanManagement,FIXX Static code analysis is the most popular approach to detect
reports 40 paths compared to 15 by TaintRadar. vulnerabilities in PHP applications [21], [22], [23], [24], [25],
[26], [1], [2], [3], [4], [5], [6], [7], [8], [9], [10], [11], [27].
Takeaway 3: By pruning infeasible and redundant taint paths, In particular, taint-based analysis suffers from a considerable
TaintRadar reduces analyst triage effort while preserving vul- falsepositiverate,requiringmanualverificationthroughdetec-
nerability recall, improving the practical usability of static vul- tion triage. On the other hand, TaintRadar uses multiple flow
nerability detection.
analysistoinfersemanticallyrichcontexttoimprovedetection
coverage while minimizing false positive detection.

<!-- page 11 -->

11
WebApplication CodeInjection CommandExecution FileInclusion SessionFixation FileAccess SQLInjection XSS
AdvocateOffice 0 0 0 0 0 36 360
AutomatedEnrollment 0 0 0 0 0 65 4311
BloodBankSystem 0 0 0 0 0 37 36
ClanSphere 2 0 8 2 54 20 172
CodeAstro 0 0 0 0 0 23 365
Collabtive 3 0 0 0 0 1 2
Covid19tms 0 0 0 0 0 19 5
DailyExpenseTracker 0 0 0 0 0 10 8
DairyFarmManagement 0 0 0 0 0 27 95
FruitsBazar 0 6 0 0 0 60 245
EngineersOnlinePortal 0 0 0 0 0 204 4599
FantasticBlog 0 0 1 0 2 6 105
HospitalManagementSystem 0 0 0 0 6 46 687
Keerti 0 0 0 0 0 5 0
LoanManagement 0 0 0 0 0 19 62
SEOPanel 0 0 0 0 3 15 234
Tailor 0 93 0 0 0 19 562
VisitorManagement 0 0 0 0 0 7 5
TABLE IV: Vulnerability detection results of TaintRadar beyond XSS and SQLI across PHP applications.
SQLInjectionVulnerabilities XSSVulnerabilities
Language AppName VanillaJoern +Dataflow +Sanitization VanillaJoern +Dataflow +Sanitization +Database
AdvocateOffice 39 84 36 30 15 17 360
CodeAstro 30 60 23 26 16 15 365
Collabtive 7 37 1 9 17 2 2
PHP
FruitsBazar 29 119 60 131 97 53 245
EngineersOnlinePortal 214 429 204 86 282 82 4599
Tailor 50 84 19 21 18 15 562
TABLE V: Ablation study of SQL Injection and XSS total paths across different components of TaintRadar.
Language Approach Acc F1 Prec Rec FPR
defense, finding that most frameworks fall short. In turn,
KAVe[1] 56.00 38.00 60.00 27.00 17.00
WAP[20] 43.00 33.00 37.00 29.00 44.00 Su et al. [33] propose a sanitizer-centric PHP code analysis,
PHP1 Pixy[8] 66.00 68.00 64.00 73.00 41.00
significantly reducing false positives. TaintRadar extends the
PHPCorrector[18] 53.00 3.00 86.00 2.00 0
TaintRadar 80.14 77.65 76.44 80.14 7.12 sanitization support for vulnerabilities other than XSS.
TABLE VI: Performance comparison of TaintRadar with Database Integration: Sadun Haq et al. [12] use database
previousvulnerabilityanalysistoolsontheSARDbenchmark. constraints to reduce false positives in container scanning.
We extend their approach by labeling queries at the node-
level with relevant information and augment the data flow
Code Property Graph Analysis: Backes et al. [17] ex- edgesaccordinglytoachievebothimprovedcoverageandfalse
tended CPG traversals for PHP vulnerability detection. Tain- positive reduction.
tRadarbuildsextendsthepipelinetoincludesanitizationanal-
ysis, database integration, and object-dependency traversal. VII. CONCLUSION
Alhuzali et al. developed NAVEX [13], a tool that combines This paper presented TaintRadar, a static analysis frame-
static and dynamic analysis for exploit generation. While work that advances taint-style vulnerability discovery by en-
TaintRadar doesn’t perform dynamic analysis, it achieves richingCodePropertyGraph(CPG)representationswithdeep
greater coverage with minimal false positives. Wi et al. [28] semantic context. Through vulnerability-typed sanitization,
proposegraphisomorphismtoidentifybugs.However,slicing persistence-aware dataflow tracking, and object-aware inter-
the graph leads to information loss, a major reason why procedural dependency analysis, TaintRadar addresses key
TaintRadarconsiderstheentireCPG.Zhaoetal.[14]enhance limitationsofpriorCPG-basedapproaches,includingdatabase
PHP vulnerability detection with improved call graphs and blindness, coarse validation modeling, and shallow object
taint tracking, but their approach lacks object dependencies, handling.
sanitization filters and database constraints. Our multi-stage evaluation shows that these semantic graph
Object Dependency Graph Analysis: Chen et al. [29] augmentations yield clear empirical benefits. On standard
introduced Object Dependency Graph (ODG) to represent benchmarks, TaintRadar outperforms state-of-the-art static
data flow across object attributes. Najumudheen et. al [30] analyzers, achieving high accuracy and F1 scores while main-
appliedODGfortestcoverageanalysisviaacall-basedsystem taininglowfalsepositives.Onlarge-scalereal-worldPHPsys-
dependencegraphtocoverdependencies,flow,callgraphs,and tems,TaintRadarre-discoveredthevastmajorityofhistorical
inheritance. Li et al. [31] implemented ODG for vulnerability CVEs and uncovered 29 confirmed zero-day vulnerabilities.
discovery in Node.js to resolve objects and variables across Crucially, TaintRadar achieves high recall without caus-
differentscopes.OurapproachextendspreviousworkonODG ing path inflation. Compared to existing tools, it prunes
by integrating it with the application’s CPG to enrich the unreachable flows and reduces redundant reports, lowering
context used for vulnerability detection. analysttriageeffort.Ablationresultsfurtherconfirmthateach
Sanitization Support: Weinberger et al. [32] analyze in- graph augmentation layer contributes measurably to detection
consistencies between web framework sanitization and XSS coverage or precision. Overall, TaintRadar demonstrates that

<!-- page 12 -->

12
context-rich, compositional semantic layers are essential for [12] M. S. Haq, A. S. Tosun, and T. Korkmaz, “Lucid: A framework for
precise, scalable, and practical static taint analysis. reducing false positives and inconsistencies among container scanning
tools,”2024.[Online].Available:https://arxiv.org/abs/2405.07054
[13] A. Alhuzali, R. Gjomemo, B. Eshete, and V. N. Venkatakrishnan,
VIII. ETHICALCONSIDERATIONS “NAVEX: precise and scalable exploit generation for dynamic web
applications,” in 27th USENIX Security Symposium, USENIX Security
In this work, we follow the responsible disclosure principle 2018,Baltimore,MD,USA,August15-17,2018.,2018,pp.377–392.
[14] C. Zhao, T. Tu, C. Wang, and S. Qin, “Vulpathsfinder: A static
in reporting zero-days. For all newly discovered vulnerabili-
method for finding vulnerable paths in php applications based on
ties, we submit the exploits to the respective vendors and to cpg,” Applied Sciences, vol. 13, no. 16, 2023. [Online]. Available:
the MITRE Corporation to request CVE IDs. https://www.mdpi.com/2076-3417/13/16/9240
[15] NationalInstituteofStandardsandTechnology(NIST),“NISTSoftware
When reporting to a vendor, for each zero-day, we include
Assurance Reference Dataset (SARD),” 2023, accessed December 15,
the description of the vulnerability with details of the vulner- 2023.[Online].Available:https://samate.nist.gov/SARD/
abilitytype,thepartofthecodebaseaffected(e.g.,filename), [16] F.Yamaguchi,N.Golde,D.Arp,andK.Rieck,“Modelinganddiscover-
ingvulnerabilitieswithcodepropertygraphs,”in2014IEEESymposium
proof-of-concept that demonstrates how the vulnerability is
onSecurityandPrivacy,2014,pp.590–604.
exploited, and suggestions for patching the vulnerability. [17] M. Backes, K. Rieck, M. Skoruppa, B. Stock, and F. Yamaguchi,
WhenreportingavulnerabilitytotheMITRECorporationto “Efficient and flexible discovery of php application vulnerabilities,” in
2017IEEEEuropeanSymposiumonSecurityandPrivacy(EuroS&P),
obtainCVE-ID,weorganizedthedetailsofthevulnerabilities
2017,pp.334–349.
per application in a repository accessible only to the CVE [18] R. Morgado, I. Medeiros, and N. F. Neves, “Towards web application
validation team. We will continue to evaluate TaintRadar on security by automated code correction,” in International Conference
on Evaluation of Novel Approaches to Software Engineering,
more PHP applications and report new vulnerabilities to the
2020. [Online]. Available: https://api.semanticscholar.org/CorpusID:
vendor and MITRE as described above. 218682970
[19] N. P. Thimmaiah, Y. J. Dave, R. Gjomemo, and V. Venkatakrishnan,
“Fixx:Findingexploitsfromexamples.”
REFERENCES [20] I. Medeiros, N. Neves, and M. Correia, “Detecting and removing web
application vulnerabilities with static analysis and data mining,” IEEE
[1] R. Ramires, A. Respício, and I. Medeiros, “Kave: A knowledge-based TransactionsonReliability,vol.65,no.1,pp.54–69,2016.
multi-agent system for web vulnerability detection,” in 2024 IEEE [21] F. Yu, M. Alkhalaf, T. Bultan, and O. H. Ibarra, “Automata-based
InternationalConferenceonWebServices(ICWS),2024,pp.489–500. symbolic string analysis for vulnerability detection,” Formal Methods
[2] Z. Jiazhen, Z. Kailong, Y. Lu, H. Hui, and L. Yuliang, “Yama: in System Design, vol. 44, pp. 44 – 70, 2013. [Online]. Available:
Precise opcode-based data flow analysis for detecting php applications https://api.semanticscholar.org/CorpusID:17856633
vulnerabilities,” 2024. [Online]. Available: https://arxiv.org/abs/2410. [22] M.Samuel,P.Saxena,andD.Song,“Context-sensitiveauto-sanitization
12351 inwebtemplatinglanguagesusingtypequalifiers,”102011,pp.587–
[3] P.LiandW.Meng,“Lchecker:Detectingloosecomparisonbugsinphp,” 600.
inProceedingsoftheWebConference2021,ser.WWW’21. NewYork, [23] Y.-W. Huang, F. Yu, C. Hang, C.-H. Tsai, D.-T. Lee, and S.-Y.
NY,USA:AssociationforComputingMachinery,2021,p.2721–2732. Kuo, “Securing web application code by static analysis and runtime
[Online].Available:https://doi.org/10.1145/3442381.3449826 protection,” in Proceedings of the 13th International Conference on
[4] G. Wassermann and Z. Su, “Sound and precise analysis of web World Wide Web, ser. WWW ’04. New York, NY, USA: Association
applications for injection vulnerabilities,” in Proceedings of the 28th for Computing Machinery, 2004, p. 40–52. [Online]. Available:
ACM SIGPLAN Conference on Programming Language Design and https://doi.org/10.1145/988672.988679
Implementation, ser. PLDI ’07. New York, NY, USA: Association [24] A. Doupé, B. Boe, C. Kruegel, and G. Vigna, “Fear the ear:
for Computing Machinery, 2007, p. 32–41. [Online]. Available: discovering and mitigating execution after redirect vulnerabilities,”
https://doi.org/10.1145/1250734.1250739 in Proceedings of the 18th ACM Conference on Computer and
[5] F.Sun,L.Xu,andZ.Su,“Staticdetectionofaccesscontrolvulnerabili- Communications Security, ser. CCS ’11. New York, NY, USA:
tiesinwebapplications,”inProceedingsofthe20thUSENIXConference Association for Computing Machinery, 2011, p. 251–262. [Online].
onSecurity,ser.SEC’11. USA:USENIXAssociation,2011,p.11. Available:https://doi.org/10.1145/2046707.2046736
[6] P. Saxena, D. Molnar, and B. Livshits, “Scriptgard: automatic [25] J. Dahse and T. Holz, “Simulation of built-in php features for precise
context-sensitive sanitization for large-scale legacy web applications,” staticcodeanalysis,”012014.
in Proceedings of the 18th ACM Conference on Computer and [26] D.Balzarotti,M.Cova,V.V.Felmetsger,andG.Vigna,“Multi-module
Communications Security, ser. CCS ’11. New York, NY, USA: vulnerabilityanalysisofweb-basedapplications,”ser.CCS’07. New
Association for Computing Machinery, 2011, p. 601–614. [Online]. York,NY,USA:AssociationforComputingMachinery,2007.[Online].
Available:https://doi.org/10.1145/2046707.2046776 Available:https://doi.org/10.1145/1315245.1315250
[7] C.Luo,P.Li,andW.Meng,“Tchecker:Precisestaticinter-procedural [27] V. B. Livshits and M. S. Lam, “Finding security vulnerabilities in
analysis for detecting taint-style vulnerabilities in php applications,” javaapplicationswithstaticanalysis.”inUSENIXsecuritysymposium,
in Proceedings of the 2022 ACM SIGSAC Conference on Computer vol.14,2005,pp.18–18.
and Communications Security, ser. CCS ’22. New York, NY, USA: [28] S. Wi, S. Woo, J. J. Whang, and S. Son, “Hiddencpg: Large-scale
Association for Computing Machinery, 2022, p. 2175–2188. [Online]. vulnerable clone detection using subgraph isomorphism of code
Available:https://doi.org/10.1145/3548606.3559391 property graphs,” in Proceedings of the ACM Web Conference
[8] N.Jovanovic,C.Kruegel,andE.Kirda,“Pixy:astaticanalysistoolfor 2022, ser. WWW ’22. New York, NY, USA: Association for
detectingwebapplicationvulnerabilities,”in2006IEEESymposiumon Computing Machinery, 2022, p. 755–766. [Online]. Available: https:
SecurityandPrivacy(S&P’06),2006,pp.6pp.–263. //doi.org/10.1145/3485447.3512235
[9] J. Dahse, N. Krein, and T. Holz, “Code reuse attacks in php: [29] J.-L.Chen,F.-J.Wang,andY.-L.Chen,“Anobject-orienteddependency
Automated pop chain generation,” ser. CCS ’14. New York, NY, graph for program slicing,” in Proceedings. Technology of Object-
USA:AssociationforComputingMachinery,2014,p.42–53.[Online]. OrientedLanguages.TOOLS24(Cat.No.97TB100240),1997,pp.121–
Available:https://doi.org/10.1145/2660267.2660363 130.
[10] J. Dahse and T. Holz, “Static detection of second-order vulnerabilities [30] E.Najumudheen,R.Mall,andD.Samanta,“Adependencegraph-based
inwebapplications,”inProceedingsofthe23rdUSENIXConferenceon representation for test coverage analysis of object-oriented programs,”
SecuritySymposium,ser.SEC’14. USA:USENIXAssociation,2014, SIGSOFTSoftw.Eng.Notes,vol.34,no.2,p.1–8,feb2009.[Online].
p.989–1003. Available:https://doi.org/10.1145/1507195.1507208
[11] A. W. Marashdih, Z. F. Zaaba, and K. Suwais, “An enhanced static [31] S. Li, M. Kang, J. Hou, and Y. Cao, “Mining node.js vulnerabilities
taintanalysisapproachtodetectinputvalidationvulnerability,”J.King via object dependence graph and query,” in 31st USENIX Security
Saud Univ. Comput. Inf. Sci., vol. 35, no. 2, pp. 682–701, 2023. Symposium, USENIX Security 2022, Boston, MA, USA, August 10-12,
[Online].Available:https://doi.org/10.1016/j.jksuci.2023.01.009 2022,K.R.B.ButlerandK.Thomas,Eds. USENIXAssociation,2022,

<!-- page 13 -->

13
pp. 143–160. [Online]. Available: https://www.usenix.org/conference/
usenixsecurity22/presentation/li-song
[32] J.Weinberger,P.Saxena,D.Akhawe,M.Finifter,R.Shin,andD.Song,
“A systematic analysis of xss sanitization in web application frame-
works,”inComputerSecurity–ESORICS2011,V.AtluriandC.Diaz,
Eds. Berlin, Heidelberg: Springer Berlin Heidelberg, 2011, pp. 150–
171.
[33] H.Su,L.Xu,H.Chao,F.Li,Z.Yuan,J.Zhou,andW.Huo,“Asanitizer-
centricanalysistodetectcross-sitescriptinginphpprograms,”in2022
IEEE33rdInternationalSymposiumonSoftwareReliabilityEngineering
(ISSRE),2022,pp.355–365.
