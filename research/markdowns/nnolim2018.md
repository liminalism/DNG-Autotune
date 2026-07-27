<!-- Converted from nnolim2018.pdf — 24 pages -->

## Page 1

### Optik
jou rn a l h om ep ag e : w w w . e l s e v i e r . d e / i j l e o
### Original research article
### An adaptive RGB colour enhancement formulation for
### logarithmic image processing-based algorithms
### U.A. Nnolim
Department of Electronic Engineering, University of Nigeria, Nsukka, 410001, Enugu State, Nigeria
| a r t i c l e | i n f o | a b s t r a c t |
|---|---|---|
| Article history: |  | This paper presents an effective colour enhancement framework for statistical and logarith- |
| Received 13 August 2016 |  | mic image processing (LIP)-based enhancement algorithms. The proposed approach |
| Received in revised form 9 August 2017 |  | the fusion of partial, multiple computed luminance channels with colour image channel |
a r t i c l e i n f o a b s t r a c t
Article history:
statistics   obtained   from   the   input   colour   image   for   adaptive   colour   enhancement.   The
posed   scheme   does   not   modify   the   image   intensity   channel,   avoiding   colour   fading   typically
observed   in   colour   images   processed   with   conventional   algorithms.   The   colour   enhance-
ment   scheme compensates   for   the   weaknesses   of   greyscale-based   contrast   enhancement
and   illumination normalization   algorithms   by   focusing   on   preserving/restoring   or   enhanc-
Adaptive colour correction ingcolour. Theproposedsystemavoidstheconversiontocomplex, non-linearcolour Nonlinear colour space models suchasHSIandHSVwhileproducingsimilarresultswithoutmanualadjustmentof Generalized un-sharp masking eters. Additionally, anadaptiveschemefordetectionofimageswithunbalancedcolour unevenilluminationiscombinedwiththeproposedsystem. Resultsshowthattheproposed scheme augments colour results of greyscale-based contrast enhancement algorithms is relatively less complex compared to most algorithms in the literature. © 2017 Elsevier GmbH. All rights reserved.
1. Introduction
The continued growth of high definition digital colour media and devices for their acquisition, transmission, processing and storage has precipitated the necessity for novel, effective and efficient colour image enhancement algorithms. algorithms must utilize relatively low hardware resources to fit within the real-time digital hardware implementation constraintsofembeddedsystemsforsuchdevices, whichprocessgreatamountsofimageandvideodataathighresolutions. Moreover, unwanted and unavoidable processes that manifest during the acquisition and transmission stages are still presentinelectronicdevices. Theseinclude; blurring, noise, non-idealilluminationconditions, distortions, limitations dynamic range and display capabilities of electronic devices [1,2]. Several algorithms have been proposed to address problems, however, the majority are suited to greyscale images. Greyscale, histogram-based algorithms perform poorly when processing images with uneven illumination [1–4]. versely, standardlogarithmicimageprocessingapproachesalsoyieldsubparresultsandatbestproducefadedimages processing faded or bright, low contrast images [4,3]. In addition, these algorithms developed for greyscale image enhance-mentdonotworkwellforRGBcolourimagesduetonon-linearityandvectornatureoftheRGBcolourchannelsthatcomprise the image [1,2]. Thus, processing individual RGB image channels usually leads to colour distortion or fading and numerous schemes have been devised in the literature to rectify this deficiency. These include colour restoration functions
E-mail address: uche.nnolim@unn.edu.ng
https://doi.org/10.1016/j.ijleo.2017.09.102 0030-4026/© 2017 Elsevier GmbH. All rights reserved.

---

## Page 2

effectiveness of the non-linear colour spaces lie in their ability to decouple the Intensity or Value channel from the Hue Saturation channels, which are all components embedded within the individual Red, Green and Blue image signals [1,2]. Currently, most recent work on colour image enhancement has focused on the non-linear colour space alternatives, which are consistent and proven in their performance. These include combining HSI combined with Homomorphic [8] and HSV colour spaces with Contrast Limited Adaptive Histogram Equalization (CLAHE) [10,11] and Genetic Algorithm or Particle Swarm Optimization (PSO) techniques [12], Dynamic Stochastic Resonance (DSR)-based image enhancement [13,14] and Tone mapping algorithms [15–19], Generalized Unsharp Masking (GUM) [11] and Probability-based illumina-tion normalization [20] algorithms. However, algorithms such as Generalized Histogram Equalization (GHE) and Adaptive Histogram Equalization (AHE) [2] yield worse results in HSI or HSV space, leading to the gamut problem described by and Murthy in their work [21]. They also proposed a technique for avoiding gamut problem after enhancement without resorting to colour coordinate transforms. The recent work on fast hue and range preserving histogram specification functions by Nikolova and Steidl [22,23], utilize these foundations for their proposed improved algorithm. Additionally, the proposed LIP algorithm by Patrascu and Buzuloiu [24], is also hue preserving, though its adaptive nature requires solution of a set of relatively complex equations to obtain particular image parameters for enhancement. Alternative colour enhancementandpreservingtechniquesincludetensor-basedmethodsusingaQuaternionDiscreteFourierTransform histogram modification equalization-based methods in 1D [26] and 3D spaces [27], development of a colour LIP colour rection framework [28] and multi-exposure homomorphic filtering [17]. Recently, works utilizing alternative LIP models [29] combining the Generalized Un-sharp Masking (GUM) [11,29] and probabilistic models for simultaneous illumination and reflectance estimation have been reported [20]. All the aforementioned alternatives still require colour conversion to HSV/HSI colour space to avoid/minimize the tortion or colour fading resulting from illumination correction operations in RGB space. This also adds increased hardware resource usage in digital hardware system implementations [3,30]. Furthermore, even when some of the algorithms implemented in the HSI and HSV colour space, there is usually not much colour improvement in addition to faded colours observed after processing. Furthermore, any unregulated modification of the hue and saturation affects the final colour theticsofthecolourimagewhenconvertedbacktoRGBspace. Thus, itisvitalthattheproposedsystemavoidsthedistortion of colours. Theprimarymotivationoftheproposedalgorithminthisworkwastoaddresstheissuesofcolourdistortion, automation and consistency of results in implementation. This is because most of the conventional approaches yield inconsistent colour results, are highly complex and not usually adaptive or automated, requiring manual adjustments of several parameters and selection of various functions. Even with canonical values, some of the earlier schemes do not yield best results images, lacking generality in application. In contrast, the proposed system yields consistent results as expected due theoretical foundations supported by extensive experimentation. The rest of the paper is outlined as follows; the second section highlights the key contributions, features and objectives oftheproposedscheme. Thethirdsectiondiscussesrelevantrelatedworksindetailandtheirlimitations. Thefourthsection discusses the details of the proposed colour enhancement function and the justification for its implementation with sentation of theoretical formulation. The fifth section presents experimental results of the proposed scheme and its colour augmentationabilityinadditiontocomparisonswithotheralgorithmsfromtheliterature. Additionalpresentationsinclude the modification of a previously computational complex algorithm by replacing a non-linear colour space converter the proposed colour enhancement algorithm. Additional replacement of the edge preserving component of the contrast enhancement algorithm with a less complex but more effective alternative, reduced the run-time and complexity algorithm. The sixth section presents a brief discussion and possible applications and hardware realization possibilities the proposed system. The final section presents the conclusions and a comprehensive summary of the work.
2. Key contributions, features and objectives
The key and novel contributions of this work include:
• Adaptive, combined fusion of intensity and value components to enhance colour rendition of processed colour images using weighted combination computed from colour channel statistics. • Colour restoration of processed colour images with distorted or degraded colours using computed modal ratios from three colour channels. • Useofprobabilitymassfunction(PMF)tocomputestandarddeviationfrommode(SDFM)andmeanofmodalratio(MOM) parameters as inputs in the implemented system, ensuring adaptive and automated colour enhancement/restoration. • Additional scheme for detecting dark images or images with dominant colour cast based on statistical contrast measure and entropy values used as inputs to a classifier for decision-based colour enhancement processing.
The key objectives and features include:

---

## Page 3

• Improving colour enhancement and colourfulness by regulated dilation without upsetting colour balance. • Algorithm yields improved colour images, matching and/or surpassing results of HSI and HSV-based approaches avoiding their complex and extensive colour coordinate forward and reverse transformation functions. • The developed algorithm is fast, feasible and practical for hardware implementation due to its point-wise nature operation. • Theproposedalgorithmcanalsobemodifiedtomimicresultsofanycolourspaceconversiontechniquewithoutperforming the actual full colour space conversion.
3. Prior related works
TheilluminationcorrectionalgorithmstreatedinthispaperarelimitedtotheLIPandSLIPmodelswithsomediscussions about the log-ratio (LR) based models. However as noted, these algorithms generally have a number of limitations, include; minimal edge and contrast enhancement, inability to perform consistent colour correction, loss of details in image regions and inability to enhance faded, low contrast images. The colour fading and distortion aspect is addressed in this work and solutions to the colour fading problem from the literature are included for comparison. Furthermore, drawbacks of these approaches are also briefly discussed with added references for further details.
3.1. Logarithmic image processing (LIP) and symmetric LIP (SLIP) models
Theilluminationcorrectiontermdiscussedhereisbasedonavariantofthelog-ratio(LR), logimageprocessing(LIP) andsymmetricLIP(SLIP)model[29]. TheRetinex-basedalgorithmsarederivedfromtheLRmethodwhiletheHomomorphic filter is a variation of the model. The LR and the LIP values are defined in the range: (0, L-1), where L is the number of defined grey levels. The operations for the LIP model [11,29] are;
### (
### )
### (
### a ab Ii (x, y))
$$ a = −Rln 1 − $$
; a ⊕ b = a + b − ( ) ;  ⊗ a = R − R 1 − R R R
### (
### )
### (
### )
b a − b
$$ = −1 ⊗ b = −R $$
b ; a  b = a ⊕ (b) = R R − b R − b
In (1) and (2), ⊕,  and ⊗ are vector addition, vector subtraction and scalar multiplication operations in the LIP domain, respectively. Thetermsaandbarevectorswhileisascalarvalue. Ristherangeoftheimagevector, definedinthedomain, R) and (0, (a) is a LIP-based transformation function for vector a. Conversely, the operations for the SLIP model [29]
### (
### )
| [ | ( |
|---|---|
|  | ) |
### |a| |a| 1|b|)2]
a = −R ∗ sgn a
$$ 1 − $$
; a  b = R ∗ sgn (a + b 1 − 1 −
( ) ( ) ln ) 1 −
R
R
R
sgn ( b )
Where in (3), the exponents,  = and  = 1 sgn(a+b) 2 sgn(a+b). Also;
### [
### ]
### [
### ( |a|)||( |b|)|−1|]
  a = R ∗ sgn a 1 − 1 − ( ) ; b = −1  b = R ∗ sgn(−b) 1 − 1 − R R
| [ | ( |
|---|---|
|  | ) |
### |a| 1|(b)|)2]
 b = a  b = R ∗ sgn a + b
$$ 1 − 1 − $$
a ( ) ( ( )) 1 −
R
R
(
)
| a |
$$ −1 − $$
(a) = R ∗ sgn(a) 1 − e R
In(3)–(6), , andarevectoraddition, vectorsubtraction{andscalarmultiplicationoperationsintheSLIPdomain −1, x < 0
| respectively. The signum function, sgn is defined as; sgn ( x ) = |  | 0 , | x = 0 . The terms a and b are vectors while  is a |
|---|---|---|---|
|  |  | 1 , | x > 0 |
| value. R is the range of the vector, defined in the domain, (0, R) and |  |  | ( a ) is a SLIP-based forward transformation function |
| for vector a while | − 1 ( a ) is a SLIP-based inverse transformation function for vector a . |  |  |
3.2. Generalized Un-sharp Masking (GUM) algorithm
The GUM algorithm proposed by Deng [11] is composed of three essential components namely an Edge Preserving (EPF), Contrast Enhancement (CE) function and the Adaptive Gain Control function (AGC). The algorithm utilized for the is the Adaptive Median Filter (AMF) [31] and the selected CEF was the Contrast Limited Adaptive Histogram Equalization (CLAHE)[10]algorithm. TheLIP-andSLIP-GUMalgorithmsaredefinedbytheexpressions; vLIP = z ⊕ [ ⊗ (x  y)]and    x  y z [ ( )] where x, y, z and  are the input image, EPF output, CEF output and AGC output respectively.

---

## Page 4

CEandAGCcomponents. TheoutputsofthesecomponentsareevaluatedintheLIPandSLIPdomainsrespectively. However, as pointed out in [20], the traditional log-based approaches lead to detail loss in the extracted reflectance component. Additionally, the traditional LIP GUM leads to out of range errors for some images and have to be corrected with the approach [29]. Furthermore, there is still loss of detail in the GUM outputs in addition to little colour improvement despite the fact that the operations are performed usually on the intensity component of the HSI or HSV transformed image. This in addition to the AMF and CLAHE, which are highly computationally complex as well. Thus the exploration of comparable, less complex algorithms to replace the AMF in the GUM forms part of the improvements of the work. Consequently, utilize relatively less complex but highly effective algorithms for the EPF component of the GUM.
3.3. Colour enhancement approaches
The key, relevant, related works on colour enhancement include the Colour restoration routine [5], ratio rule [6,7], linear colour spaces [2], discrete cosine transform (DCT)-based method [32] and hue preserving algorithms [21,23]. scheme has its own unique features resulting in advantages and disadvantages.
3.3.1. Colour restoration function (MSRCR) The colour restoration function for the popular MSRCR [5] is given as;
### [
### ]
˛Ii (x, y)
i=1Ii (x, y)
### [
### ]
C (x, y) = ˇlog
i 10∑s
I y = G C y .I MSRCRi(x, ) i (x, ) MSRi(x, y) + b
### ∑S
x, I x, I x, IMSRi( y) = ws [log10 ( i ( y)) − log10 ( i ( y) ∗ hs (x, y))]
hs (x, y) = Ke s
x2+y2 c2
In Eqs. (7)–(10), i=1,...N where N is the number of bands or channels in the input image, s=1,..S, where S is the number scales, w is the weight of the scale, I y the ith band of the input RGB colour image, h s i (x, ) is s (x, y) is the Gaussian surround function or spatial filter kernel, I y the ith band of the output image of the MSR algorithm while I MSRi(x, ) is MSRCRi(x, y) is ith band of the final output image of the MSRCR algorithm after applying the colour restoration function. Lastly, , ˇ, b are constants which are termed canonical gain-offset parameters to obtain the best results for the MSRCR [5] which highly computationally complex. The colour restoration function in (7) utilizes decoupled, normalized R, G and B channels of the original image (the being converted to the logarithm domain) to reduce effects of inter-channel dependency [5].
3.3.2. Ratio rule The ratio rule was developed by Seow, et al. [7,6] and is a state-space associative memory-based neural network. networkemploysaline-of-attraction-basedrepresentationtodescribetheRGBinter-channelcolourrelationshipswithin image [6,7]. After training the network with the initial colour arrangement prior to processing, the system recalls the colourstructureoftheprocessedimagebyiteration[6,7]. Thisinformationisusedtorecalibratethecoloursoftheprocessed faded RGB colour image, effectively performing restoration. The homomorphic filter [1,2] was the image enhancement algorithm used for the processing of the RGB colour image, leading to the faded result that required restoration. The Rule scheme is evidently computationally complex [33], and it is not clear what the implications would be if the size the images increases considerably in addition to increase in iterative operations. Furthermore, the algorithm and its results were not extensively compared to existing colour enhancement alternatives. A similar colour retrieval and enhancement scheme is proposed by Iqbal, et al. for surveillance images [34].
3.3.3. Non-linear colour spaces Luma-based approaches have become the norm in colour enhancement-based image processing, due to the established colour coordinate transforms and successes in colour segmentation applications utilizing non-linear colour spaces. Studies carriedoutusingthissolutionhaveshownthatthesystemisfeasibleandhardwarearchitecturesutilizingtheschemes been reported in the literature [8,9]. These include the HSI and HSV colour spaces and in the latter, the hue is defined the rotation angle between the Red plane and the origin [1–3]. The saturation is the distance from any point in the colour space system to the colour surface [1–3], while the value (V) represents the intensity (or luminosity) channel attributes the image. The equations for the RGB to HSV transformation [1,2] are given as;
$$ V = max (R, G, B) ; S = V − min (R, G, B) $$

---

## Page 5

### ⎨
### ⎬
B − R
| respectively. The signum function, sgn is defined as; sgn ( x ) = |  | 0 , | x = 0 . The terms a and b are vectors while  is a |
|---|---|---|---|
| H = | 2 + | ; G = V |  |
| ⎪ | S |  | ⎪ |
| ⎪ |  |  | ⎪ |
| ⎩ | R − G |  | ⎭ |
|  | 4 + | ; B = V |  |

$$ H = $$
+
S  
HSI   colour   space   was   based   on  
  or   circle   representation   [1,2].  
  however,   their   mathematical
hexagon   model   and   line   segments
value   of   120   ◦   in   the   colour   circle  
R + G + B min (R, G, B)
$$ I = $$
; S = 1 − 3 I
### ⎛
### ⎞
### {
### }
; ifB ≤ G −1⎝ (R − G) + (R − B)
$$ H = 360◦−  B > G ;  = cos 2√((R − G)2 + (R − B)(G − B))⎠ $$
The HSI colour conversion yields superior results compared to the HSV colour space [30,3,8]. Large LUTs are normally used for the hue computation in HSI space, though this becomes impractical for images with larger bit depths greater eight bits per pixel (bpp) on digital hardware systems with limited memory. There are numerous alternative equations for calculating the hue component [3,30], which is the most crucial aspect. The search for alternatives is partly due challengesencounteredinaccuratelycomputingthehuechannelindigitalhardwaredevicessuchasFPGAsusingfixed [3,30]. However, a hardware architecture solution utilizing a proven alternative formula, resulting in the full HSI converter FPGA hardware architecture has been reported in recent literature [3,30]. To date, this is still the only publicly known available implementation. Basedonworkintheliterature, theHSVvariantisthemostcommondigitalhardwareimplementationofnonlinear space converters [9,3,35,18]. This is due to its relative ease of implementation compared to the HSI colour space conversion in fixed-point digital hardware [30]. Since the Hue is the most important aspect of the human visual system, on which HSI and HSV colour spaces are based, any distortions in computation would affect the final result, even without processing [3,30]. In the HSV case, hue computation is much more straightforward, using multiplexer and basic rational operations. However, utilizing the non-linear colour space converters result in a speed performance bottleneck in hardware theintensitychannelisprocessed(thoughthisisanadvantageintermsofreducedcomputationalloadandcomplexity, imageprocessingalgorithmitselfisrelativelyhighlyinvolved). ThisisbecauselatencyincreasessincetheHueandSaturation channels must be synchronized with appropriate delays to ensure that they are correctly combined with the corresponding processed intensity channel for accurate conversion to RGB space after processing. This thwarts parallelization efforts to the dependency of all three channels on each other for the reverse conversion. Even pipelining techniques reach performance ceiling beyond which there is no further improvement in speed of the design. Latency is increased along with throughput as a result but this delay adds to the speed of the reverse conversion. Thus, no matter how fast image processing algorithm or the colour space conversions are, they all depend on the amount of delay required to accurate synchronization and conversion to RGB space.
3.3.4. DCT-based colour enhancement This scheme was proposed by Mukherjee and Mitra for colour enhancement in DCT domain [32]. The approach converts the RGB image to the YCbCr or YUV colour space and computes the DCT of each component [32]. Three main processes are then performed on these transformed components, namely; local background illumination adjustment, local contrast preservation and colour preservation [32]. In spite of its apparent simplicity, there is DCT computation, block splitting merging steps and several parameters to consider in addition to a blocking artefact removal (BAR) algorithm [32]. Also, numberofparameterstotuneincreases, dependingonthefunctionchosenforilluminationadjustment. Thus, thealgorithm is not automated, requires user specified parameters, a selected function and option of using BAR algorithm if blocky appear in the processed images.
3.3.5. Hue and range preserving colour enhancement Initially, theworkbyNaikandMurphyproposedacolourimageenhancementalgorithmthatavoidedthegamutproblem [21]. Recently, Nikolova and Steidl proposed a fast hue and range preserving histogram specification algorithm to mitigate the gamut problem that occurs after RGB colour image enhancement [23]. Rather than processing each individual channel, they convert the image to the HSI colour space and map the intensity channel to an ideally chosen histogram using ordering algorithm developed by [36]. However, the algorithm is not automated and performs best with user specified parameters and and may yield inconsistent results.

---

## Page 6

Fig. 1. (a) Original image and colour profile plot (balanced profile) (b) Colour distorted image and colour profile plot (unbalanced profile).
4. Proposed colour enhancement approach
Based on analysis of related works, there is need for a scheme that combines the effectiveness and advantages of the and HSV colour space while avoiding their weaknesses. Such a scheme should be able to handle most images presented yield good colour enhancement/rendition regardless of the damaging effects of the prior applied image contrast enhance-mentalgorithm. Thisisinadditiontoavoidanceofdistortionordegradationoftheimagepixelintensityvaluesintheprocess of colour rendition. The requirements of this system include;
• Perform on par or (if possible) better than the HSI and HSV colour converters • Inherit their strengths (decoupling intensity, thus becoming contrast agnostic) and eliminating their weaknesses. • Should not distort colour balance of the R, G and B channels. • Suitable, practical and amenable to hardware implementation. • Should be adaptive and avoid manual parameter adjustments. • Should be generic and work with most algorithms it is combined with.
The general form of the proposed algorithm consists of a contrast enhancement transfer function, T f x, y
{ } ( ( )) and colour
x,   y =   I
enhancement ( ( )) of ( ) i (x, y) ; i = 1toN with the number of channels,
$$ N=3 expressed in the following form for each channel; $$
I y = C I y .T I ifinal(x, ) ( i (x, )) ( i (x, y))
Assume the RGB colour cube as a container, within which all possible colour image configurations will fit. Thus, we a colour plot of the relationship between the R, G and B channels in a colour image. If we consider the image pixels particles, and that the cohesion of these particles taken as a function of their distances apart from their mean, then we measure the correlation of the pixels to each other. Ideally, highly correlated particles are closer (greater cohesion) those that are further apart have less correlation (lower cohesion) with each other. However, since the distribution of image colour pixels do not follow a normal distribution and will have highly skewed mean vales, the usage of the mode probabilitymassfunction(PMF)givesabetterestimate. Thus, weinitiallyassumethatthecolourfulnessorcolouraesthetics that is visualized by the human visual system and interpreted by the brain could be perceived as a measure of the cohesion or correlation of these particles defined by their closeness to their modal ratio values. If the R, G and B particles are slightly apart, the colour balance is slightly modified. However, if the distance increases between the particles and the centre the probability mass distribution, the colour fades or distorts and colour balance is lost (destabilization). This balance inherent colour relationship between the R, G and B channels of colour images. We proceed to test these assumptions actual experiments to verify, modify, discard or re-evaluate them. The figures shown in Fig. 1 illustrate this idea of a colourful (Notre Dame) [5] image and its distortion in relation RGB colour plot. In Fig. 1(b), the image is highly distorted since the R, G, B particles are scattered and some are almost

---

## Page 7

Fig. 2. (a) RGB histogram of intensity ratios of (b) original image (c) processed with adaptive modal ratio guided RGB spatial HF (modal R ratio=2.5,modal G ratio=3 modal B ratio=4).
Table 1 Sample RGB colour images and their modal ratios.
| Image | Mode (R) | Mode (G) | Mode (B) | Mode (rounded) (R) | Mode (rounded) (G) | Mode (rounded) |
|---|---|---|---|---|---|---|
| Girl [37] (a) | 2.5 | 3 | 4 | 3 | 3 | 4 |
| Swan [5] (b) | 3 | 3 | 3 | 3 | 3 | 3 |
| Notre Dame [5] (c) | ∞ | 3 | 3 | ∞ | 3 | 3 |
| StarTrek [5] (e) | 4 | 2.5 | 3 | 4 | 3 | 3 |
| RVs [38] (f) | 3 | 3 | 3 | 3 | 3 | 3 |
| Church [39] (h) | 2 | 3 | 3 | 2 | 3 | 3 |
| Sky lake [39] (i) | 3.8 | 2.7143 | 2.7143 | 4 | 3 | 3 |
| Parked car [39] (j) | 3 | 3 | 3 | 3 | 3 | 3 |
| Butterfly [39] (k) | 2 | 3 | 4.5 | 2 | 3 | 5 |
| Grandma & baby [39] (l) | 3 | 3 | 3 | 3 | 3 | 3 |
| Books1 [39] (m) | 3.8 | 2.7143 | 3 | 4 | 3 | 3 |
| Colours [39] (n) | 2.75 | 3.1429 | 3.1429 | 3 | 3 | 3 |
| Colour gamut [39] (o) | 3 | 3 | 3 | 3 | 3 | 3 |
| Scale models [39] (p) | 3.8 | 2.7143 | 2.7143 | 4 | 3 | 3 |
| Dawn dusk [39] (q) | 3 | 3 | 3 | 3 | 3 | 3 |
| Mountain peak [39] (r) | 3.0120 | 3 | 2.9880 | 3 | 3 | 3 |
| Room [39] (s) | 3 | 2.5714 | 4 | 3 | 3 | 4 |
| Woman [39] (t) | 3.5 | 3 | 2.6250 | 4 | 3 | 3 |
| Cat [39] (u) | 3.3333 | 2.8571 | 2.8571 | 3 | 3 | 3 |
| Boy on stones [23] (v) | 3 | 3 | 3 | 3 | 3 | 3 |
| Cathedral2 [23] (w) | 3 | 3 | 3 | 3 | 3 | 3 |
| Orchid [23] (x) | 3 | 3 | 3 | 3 | 3 | 3 |
| Bungalow [23] (y) | 3 | 3 | 3 | 3 | 3 | 3 |
Image Mode (R) Mode (G) Mode (B)
edge of the cube. This distortion can arise from a grey-scale algorithm with extremely high gain incorrectly applied to
colour image. Furthermore, the degradation is drastic in the high frequency regions or areas with more “energetic”
Note that the smooth, slowly varying regions (blue sky) are relatively unaffected by the distortion.
The proposed system re-establishes correlation between the various colour channels by forcing these scattered
cles closer or re-arranging the structure of the particles without upsetting the colour balance. This reduces the distance
betweenparticles, indicatingavisualplotofcolourfulness. Theproposedmethodenhancescolourimages, avoidssaturation
of bright/dark regions of images, thus suitable for both contrast enhancement and illumination normalization/correction
algorithms. We will show that considerable improvements are obtained with the proposed colour enhancement scheme.
4.1. Colour channel dominancy and distortion prediction
Initial experiments were performed to determine any discernible consistent pattern between the R, G and B channels
using normalized decoupled versions of the channels based on formulations from the literature. The sample results
initial experiment are as shown in Fig. 2(a) with the histogram of the inverse normalized RGB channels or intensity ratios
the girl image [37] processed with the intensity ratio controlled HF. Observing the modal ratios show that the Red channel
has a less common occurring value. Any attempt at using the actual modal ratio values for the red component in this
will yield erratic results as shown in Fig. 2(b) as the most occurring pixel ratio is not up to a third of the RGB colour
Thus, when using the adaptive the red channel is overcompensated, leading to oversaturation, causing the observed
bleedingeffectinFig. 2(b). TheredflowerintheimageiscompletelyindistinguishablefromtheredscarfinFig. 2(c)and
red regions are also overcompensated. Sample results in Table 1 show the modal ratios of the normalized colour channels

---

## Page 8

Fig. 3. Pie chart plots showing RGB modal ratio profiles of Girl image.
Fig. 4. (a) Image processed with HF and intensity-channel model (HSI) in RGB space (b) RGB colour plot.
of several tested images and indicate that most of the pixel values are close to the assumed model of intensity, with a of 3. ThepiechartsgeneratedinFig. 3showsthemodalratioprofile(i.e. mostcommonratio)valuesobtainedforthedecoupled, normalized R, G and B channels of the Girl image. The values generated in the plots indicate the percentage of values match the modal ratio of each channel. Rounding up of values as shown on the left hand side of Table 1 will increase respectivepercentagesforeachofthechannelsinthemodalprofiles, whichwouldleadtocolourdistortionsafterprocessing. Based on quick observation, 3 is the most commonly encountered ratio value, validating the naive intensity model from HSIbyexperimentation. Thepiechartrepresentationwaschosensinceitprovidesquickvisualcuesregardingthebalancing of R, G and B channels of a colour image, which may not be easily observed using conventional visual representations ashistogramsandimagediagrams. Thedominanceofachannelcanbeeasilydeducedduetothepiechartplotsofthemodal ratio values in each channel (Fig. 4). ThereasonforthedistortionofcoloursinthebackgroundcomponentoftheimageinFig. 2(c)isduetothenaïveadaptive schemeapplyingthecomputedmodalratioofeachchannelasafactor. Thisfavoursthelessdominantchannels(inthis R and B) and will result in some colour regions of the image being over enhanced (where modal ratio values are less than equal to 3) and punishes the more dominant channels (in this case, G) while other colour regions are slightly or moderately enhanced (where ratio values are greater than 3). This is confirmed by the saturated blue background and red hue face and scarf foreground and the RGB image modal ratio profile in Fig. 3. Thus, the modal ratio profile is a reliable indicator of the results of the blind, adaptive scheme, which are clearly undesirable.

---

## Page 9

4.2. RGB colour enhancement
Initially, thesystemwasdevelopedusingtheintensitycomponentcomputationoftheHSIcolourspaceasastarting since it provides the best colour rendition when combined with enhancement algorithms [30]. Additionally, the proposed system is less computationally complex than the colour restoration function and the ratio rule method (which is iterative). Based on approximations of the experimental results, we define the enhanced output colour image as;
### [
### ]
### Iifinal(x, y) = Ii (x,I y) .Ii’ (x, y) ; I = ∑Ni=1wiIi
Wherei=1. ., N; Nbeingthenumber∑ofN channels(N=3forRGBimage)andtheintensitychannel, Iwhilewi istheweighting factor of the ith channel such that i=1wi = 1. The weights do not have to be equal but their sum must be unity. For HSI colour space, the intensity is calculated and setting R=I1, G=I2, and B=I3 and recognising that w1 = w2 = w3 = well-known form is as shown in (17)
| I 1 + I 2 + I 3 |  | R + G + B |
|---|---|---|
| I = |  | = |
|  | 3 | 3 |
1 + I2 + I3 R + G + B
$$ I = $$
= 3 3
Thus, based on extensive experimentation, a large majority of colour images are properly enhanced using the scheme, while a few outlier images need some modification by using unequal weights for the improved colour output. The balancedimageswillyieldratiosof3foreachchannel, butotherimageswillyieldratiosgreaterthanorlessthan3for the R, G or B channels. Investigation of the degree of dispersion of pixel ratios from the modal value necessitated a measure to evaluate the standard deviation from mode (SDFM) and is defined as;
$$ √ ∑N $$
SDFM =  = i=0(Ii − m)2
N
Where m is the modal value obtained from Ii, the image intensity ratio (or inverse normalized image) distribution ith channel of the colour image. The values obtained from the R, G and B channels of the girl image are 0.6711, 13.9831 and 6.7491 respectively. The figures imply that the R channel has the lowest variance from the modal value. However, does not give a clear explanation, thus the usefulness of the modal profile, which indicates that the R channel intensity ratios are quite low. Evaluation of the maximum value in the R channel yields a value of 9 and a minimum value of 1.0233. For the G channel, maximum value is 168 and minimum value is 1.2857. For the B channel, maximum value is 225 minimum value is 1.6667. Thus, the modal values actually give the maximum value of the probability mass function, cannot be deduced from the maximum and minimum pixel intensity values across colour channels. The standard formula for computing a greyscale image from RGB space based on the luminous efficacy curve [1] is given as;
$$ I = 0.299 ∗ R + 0.587 ∗ G + 0.114 ∗ B $$
In(19), w1 =0.299, w2 =0.587, w3 =0.114andinallcasesw1 +w2 +w3 =1. Thisschemeleadstocolourbleedingforimages and high saturation in the overcompensated channel when using mainly the HSI intensity model. The HSI result has much colour and using the colourfulness parameter by Susstrunk and Hasler [40], the output colourfulness is 174.6909 (which is quite high) and relative colour enhancement is 3.9549. It is clear from the image that the colourfulness measure alone cannot not provide any information about colour aesthetics of balance as earlier assumed. Due to this oversaturation of the red channel using the HSI colour space, the Value channel from the HSV model was tested and is given as;
$$ V = max (R, G, B) $$
The results are shown in Fig. 5 with colourfulness value of 78.8977 and relative colour enhancement value of 1.7862. Though the HSV provides a better balance, there is still dominance of Red channel in regions of the image. Thus a combination of values was devised to yield an image with more balanced colour output since the value channel would reduce the oversaturation effect of the intensity channel. We obtain the intensity-value (IV) result by combining intensity and value components in the general form;
IV = ˛I + ˇV
with balanced colour output. The result of the combination with the HF is shown in Fig. 6. Comparing the image processed with HSI-HF and the RGB IV-HF shows the improvements of the model and its similarities with the HSI colour space results in RGB space. Note that the red flower and scarf features are much more discernible in the IV-model than in the results. Thus, combiningbothvalueandintensityschemesyieldsahybridmodel, whichisbetterthaneitherofthetwo, more balanced saturation by combining the strengths of each colour space model without inheriting their weaknesses.

---

## Page 10

Fig. 5. (a) Image processed with HF and value-channel model (HSV) in RGB space (b) RGB colour plot.
Fig. 6. Comparison of (a) combined intensity-value-channel version (RGB-IV-HF) and (b) HSI-HF.
It should be noted that the weighted average is one method of combining the channels and alternative methods include evaluating maximum of the intensity (I) and value (V) channels. However, by deduction and experiment, the results resembled the HSV results using maximum while a minimum yielded the HSI results. Other relationships such multiplicative-squarerootrelationshipyieldedgoodbutinconsistentresults. Thus, theweightedaveragemodelstillprovided the best and most consistent results. Based on experiments, the probability mass function (PMF) of intensity ratios obtains a maximum value around 3, which is the modal ratio value. With the results obtained, the complete colour enhancement routine can be redefined using the IV model in the general form, with the constants  and ˇ as;
### [
### ]
### [
### ]
I y I i (x, ) ’ i (x, y)
I y = .I y = .I’
ifinal(x, ) IV i (x, ) ˛I + ˇV i (x, y)
Further expansion of the formula leads to the following left-hand-side (LHS) expression in (23), using the intensity value channel calculations. If the RGB to intensity image conversion is to be employed with varying weights, the general form is shown in the right-hand-side (RHS) expression of (23);
### ⎡
[ ] ⎤
Iifinal(x, y) = ˛(R+G3+B)+ ˇ(max (R, G, B)) .Ii (x, y) = ⎣˛(∑Ni=1wiIi (x, y))+ ˇ(max (R, G, B))⎦.Ii’ (x, y)

---

## Page 11

As was earlier noted, the colours of some enhanced images are oversaturated. Consequently, we utilize the modal to control the level of colour enhancement by utilizing the reciprocal as an exponent for the colour enhancement formula. In other words, control of the saturation is obtained by modifying Eq. (23) as;
|  | ⎡ |  |  | ⎤ 1 ⁄ m |
|---|---|---|---|---|
| I i final ( x, y ) = ⎣ ( ∑ N |  |  | I i ( ) x, y ) | ⎦ .I i ’ ( x, y ) |
|  | ˛ | i = 1 w i I i ( x, y ) + ˇ (max ( R, G, B )) |  |  |
I y = ⎣ ( ) ⎦ .I’
For emulation of results from any other linear colour spaces (which are hue preserving as noted by Naim and Murthy), the expression becomes;
| ⎡ | ⎤1⁄m |
|---|---|
| I i   ( | y ) |
Iifinal(x, y) = ⎣(∑N)⎦ .Ii’ (x, y)
i=1wiIi (x, y)
This would apply to the matrix-based, linear colour space transformations such as YIQ, YUV, YCbCr, etc, where the sity/luminance notation can be appropriately substituted (though the expression may be of similar or greater complexity than the linear colour conversion operations). Finally, in practice, we select m=1 or 2 for ease of operations and ware implementation amenability, resulting in a linear or square root operation. The summary of the experiments observations is as follows;
• The initial scheme utilizing only the weighted intensity model did not yield considerable colour balancing, which achieved with saturation tuning in the HSI and HSV colour spaces. • Certain colour images match the Intensity channel model of the HSI colour conversion and yield consistent results. non-ideal colour images are more suited to the hybrid I–V model and the latter model yields good results for most images processed. • The system avoids colour space transformation and enhances the colour improvement attribute of the algorithm combined with. • The system targets the inherent relationship between the R, G and B channels and thus its generality is satisfied based experiments, indicating that the relationship between R, G and B channels is a reasonably predictable one. • Finally, it is evident that this scheme appears to take a form similar to that of the colour restoration function employed in the MSRCR and also similar to the Ratio Rule method, but with reduced complexity and no iterations or canonical parameters.
With the foundations of the colour enhancement operator elucidated, we proceed with experiments to fine tune algorithm for outlier images. After that, we then proceed with the augmentation of several conventional image contrast enhancement and illumination correction algorithms, which usually perform poorly for colour images.
4.4. Dynamic computation of the m parameter for fully adaptive colour enhancement
Itwasdiscoveredinthecourseoffurtherexperimentsthatsettingthesamevalueforthemparameterforeachofthe andBchannelsresultsininconsistenciesforsomeimages. Thisisduetothefactthattheresultsofthemvaluearedependent on the nature of the algorithm and the image being processed. For example, for some images, m=1 is adequate while others it leads to over- or under-enhancement of colours. Conversely, for other images, a value of m=1 or increasing value of m resulted in faded colours. Thus, it was imperative to obtain a standard formulation that enabled the consistent enhancement of colours that were derived from the image attributes rather than a fixed parameter. Subsequently, image statistical parameters were explored in order to obtain values for enhancement of colour images. The mean standard deviation proved unreliable and yielded highly distorted results while the mode once more proved to measure of choice for best results. Initial attempts using the mode utilized the following formulation;
R R + G + B + G + B R + G + B = g = r ; ; b = R G B
mr = mode(r) ; mg = mode(g) ; mb = mode(b)
However, this formulation yielded inconsistent results for images with unequal modal ratios. As a result, some resultsdepictedcolourenhancementorpreservation/retentionwhileothersyieldedfadedcolours, whichwasthedrawback

---

## Page 12

|  | R | G | B |
|---|---|---|---|
| r = | ; g = | ; b = | ; IV = ˛I + ˇV |
|  | IV | IV | IV |
r = g = b = ; ; ; IV = ˛I + ˇV IV IV IV
mr = mode(r) ; mg = mode(g) ; mb = mode(b)
mr + mg + mb m =
The mean value of the modal (MOM) values could then be used to enhance all three channels and preserve the colour relationships of all three channels or at worst, enhance the colour relationship rather than degrade it. However certain images yield zero values for these modal parameters for either red, green or blue channels. This skewers the mean once more leading to inconsistent results. In other words, performing a simple mean of modal values does not work well all images, leading to spurious results when either mr =0, mg =0 or mb =0. Thus, we remove all zero entries and compute mean of the remaining, non-zero entries. This ensures constant and consistent enhancement, while avoiding colour fading in all image enhancement results. Since the mode is more stable, the clustering of the colour component pixels (particles) around the mode should be consistent and this has been observed in other work [41,42].
5. Experiments and comparisons
The results in this section are presented to verify the claims made concerning the proposed colour enhance-ment/restoration algorithm (PA). The first part introduces the relevant metrics used in assessing image quality and they are interpreted in results. The second part deals with the augmentation of the colour enhancement abilities of ing greyscale image-based contrast enhancement algorithms with PA. The third and fourth section compare PA with relevant colour spaces and colour enhancement algorithms from the literature respectively. The fifth section discusses justificationforthesubstitutionoftheAMFwithintheEPFcomponentoftheSLIP-GUMalgorithmforreducedcomputational time. The sixth section deals with attempts to adaptively determine the type and degree of degradation (unbalanced colour or uneven illumination) of the input image prior to processing.
5.1. Image quality metrics
The visual and quantitative results are presented to verify the claims made about the proposed algorithm. The quanti-tative measures for evaluation include the Colourfulness (C) [40], Colour Enhancement Measurement (EMEC) [43], Average Gradient (AG) and Hue Deviation Index (HDI) [44] and entropy. The relevant mathematical expressions for these measures are shown in Eqs. (31)–(36). These are in addition to computation of mean () and standard deviation () of the image, addition to the Contrast Enhancement Factor (CEF).
$$ ∑ ∑ $$
2 1 M N 1 ∑M∑N
 MN
i=1 j=1 MN
i = 1
i
### √
AG = MN
x=1 y=1 2
√
2
2
2
$$ C = ˛ + ˇ + 0.3 ˛ + 2ˇ $$
### [
### 1 ∑k1∑k2 maxk,l (IR, IG, IB)]
EMEC = 20log k k min 1 2 k=1 l=1 k,l (IR, IG, IB)
### [
$$ 1 ∑M∑N] $$
i=1 j=1
(31) the term 2, is the variance and its square root is the standard deviation as mentioned earlier.
InEq. (34), =R−G, ˇ = R + G2 − B, , , ˇ andˇ arethemeanandstandarddeviationsofandˇ respectively
R, G and B are the red, green and blue channels of either original or processed colour image and increase in colourfulness implies colour enhancement. Human-based subjective rankings can be objectively estimated using some defined metrics [40]. Thus, thismetricisusefulsinceithadacorrelationofover90%withhumansubjectiveevaluationresultsobtained psychophysicalexperimentsaccordingtoitsoriginators[40]. Thus, weexpectthatincreasedcolourfulnessvalueofprocessed

---

## Page 13

Fig. 7. Flowchart of proposed approach.
imageswouldbeseenasmorecolourfulbymosthumans. Furthermore, resultsbyMukherjeeshowthatcolourfulness is not considerably affected by compression level though blockiness increases with increasing compression level [32]. makes the colourfulness metric a robust and reliable parameter for assessment. In (35), the RGB colour image, I = (IR, while k1 and k2 are the size of the blocks chosen for subdivision of the image, k and l are the row and column indices the image; also increased EMEC signifies improved colour. In Eq. (36), Y (i, j) is the processed hue channel and X (i, j original hue channel of HSI or HSV transformed RGB colour image. For the HDI, a lower value means that there is reduced colour distortion or deviation from the original colour signifyinggoodhuepreservationproperties(goodcohesionamongparticles)andmeasuresappropriatecolourfulness based on regulated dilation of particles. Additionally, the ratios of the values obtained from the processed and unprocessed images are obtained and provided for comparisons. If the ratios are less than 1, then there is degradation while ratio greaterthan1implyimprovementorenhancement. TheratiooftheHDIisnotcomputedsincethemetricutilizestheoriginal and processed images in computation of the deviation parameter. The flowchart for the entire experimental system setup shown in Fig. 7. The details of determination of colour balance of the input colour image and checking for adequate correction is explained in subsequent sections.
5.2. Colour enhancement of images processed with greyscale-based algorithms
This section presents results visually observed in images processed with the proposed colour enhancement scheme combined with existing illumination correction and contrast enhancement algorithms. We compare results using the image [5] processed with HSI, HSV, RGB and IV-based versions of the GHE, AHE, CLAHE, spatial HF (SHF) and frequency domain HF (FDHF), MSR, HS and GUM algorithms and shown in Fig. 8. ThevisualandnumericalresultsinFig. 8andTable2showthattheproposedcolourenhancementapproachisconsistent and improves colour rendition in the processed images. In Fig. 8, AHE, GHE and HS yield the most noticeable distortions colour while the log-ratio (LR) and logarithmic image processing(LIP)-based approaches such as the Homomorphic (SHF & FDHF), MSR and LIP-GUM yield less distortion though more colour fading is observed. However, the HF and algorithmsyieldtheleastdistortionfortheLIP-basedapproachesduetotheirexcellentdynamicrangecompressionabilities, while the MSR combines large scale and small scale features to preserve the multi-scale features of the image scene. We also compare the proposed algorithm with the results of Ratio Rule, HSI version of the spatial Homomorphic RGB-IV spatial HF, MSRCR and HSV-SLIP-GUM in Fig. 9 and results show that the algorithm yields better results than other approaches, while being relatively less complex compared to the Ratio Rule and MSRCR versions. For example, sky is much more colourful in Fig. 9(d) when compared with the other results while there is more contrast in the SLIP-GUM result due to the CLAHE contrast enhancement component. For the swan image used, the initial values for EMEC 1, Colourfulness (CMA), mean (mu i), standard deviation (sigma entropy (e i) and average gradient (AG i) are 13.43151, 15.63302, 52.18342, 62.46685, 6.564669 and 5.813808 respectively. The output values of the corresponding metrics are shown in Table 3. In both tables, the first row of each sub-table processing in RGB space without the proposed approach (RGB) while the second row in each sub-table is for processing RGB space using the proposed algorithm (RGB-IV).

---

## Page 14

Fig. 8. Swan image processed with (a) GHE (b) AHE (c) CLAHE (d) SHF (e) FDHF (f) MSR (g) HS (h) GUM in RGB domain (first and third rows) and proposed IV colour enhancement algorithm (second and fourth rows).
Table 2 Quantitative image quality measurement results for swan image processed with RGB and RGB-IV versions of the (a) GHE (b) AHE (c) CLAHE (d) FDHF (f) MSR (g) HS (h) GUM algorithms.
Version\image RC F EMEC RM RSD RE RAG HDI EMEC metrics
RGB 2.345871 0.535439 2.073571 2.46279 1.148335 1.076786 3.067447 11.31385 27.85118 RGB-IV 4.108481 0.597002 3.010375 2.320705 1.177058 1.21114 3.119015 5.534485 40.43387 (a)
2.465277 0.508135 3.831895 2.369004 1.097167 1.213991 4.309845 14.60396
RGB 51.46812
RGB-IV  
59.22205
(b)
1.628192 0.593445 1.225813 1.533186 0.953867 1.142119 2.278457 6.69303
RGB 16.46451
RGB-IV   24.58342
(c)
1.36406 0.492213 0.946139 2.044878 1.003252 1.131727 2.924515 7.645274
RGB 12.70807
RGB-IV   18.66479
(d)
1.306712 0.447208 0.701319 2.061137 0.960081 1.115542 2.326636 5.14373
RGB 9.419773
RGB-IV   14.85889
(e)
1.648464 0.973854 – 1.126875 1.047574 0.960945 2.131063 9.087314
RGB –
RGB-IV  
–
(f)
RGB 2.433331 0.573 3.25958 2.44251 1.18303 1.218645 3.060409 10.83352 43.78107 4.219846 0.635281 3.756287 2.304472 1.209953 1.215316 3.103987 5.132268
| RGB-IV | 50.45259 |
|---|---|
| (g) |  |
1.967059 0.249363 0.692045 3.140091 0.884885 1.169709 2.86375 7.819348
RGB 9.295204
RGB-IV   14.2687
(h)

---

## Page 15

Fig. 9. (a) Light-house, shoe and surfers images processed with (b) RGB HF (c) HSI-HF and (d) RGB-IV-HF and respective colour plots.
The results in Table 2 show that for colour enhancement metrics such as relative colour enhancement (RC), relative (F), relative EMEC (REMEC), the proposed colour enhancement algorithm gives consistently higher values than the standard RGB-based approaches. The colourfulness and EMEC values have increased while the HDI value has dropped considerably, indicating that the colour enhancement algorithm has reduced the hue deviation and avoided colour distortion effect various algorithms, thus the lower HDI can be seen as better cohesion of colour components. In general, the proposed scheme yields improved colour rendition when combined with grey-scale based enhancement algorithms for processing RGB colour images.

---

## Page 16

| Version \ image metrics | CMB | mu o | sigma o | ent o | AG o |
|---|---|---|---|---|---|
| RGB | 36.67305 | 128.5168 | 71.73287 | 7.068746 | 17.83355 |
| RGB-IV | 64.22798 | 121.1023 | 73.52713 | 7.950732 | 18.13335 |
RGB 36.67305 128.5168 71.73287 7.068746 64.22798 121.1023 73.52713 7.950732 RGB-IV 18.13335 (a)
38.53974 123.6227 68.53655 7.969447
RGB 25.05661
RGB-IV  
24.62635
(b)
25.45356 80.00689 59.58507 7.497631
RGB 13.24651
RGB-IV   13.3061
(c)
21.32438 106.7087 62.67002 7.42941
RGB 17.00257
RGB-IV   17.27024
(d)
20.42786 107.5572 59.97325 7.323166
RGB 13.52661
RGB-IV   14.06228
(e)
RGB 25.77048 58.80421 65.43866 6.308283 12.38959 42.22787 56.55438 65.03129 6.289551
| RGB-IV | 12.01343 |
|---|---|
| (f) |  |
38.04033 127.4585 73.90013 8
RGB 17.79263
RGB-IV  
18.04598
(g)
30.75108 163.8607 55.27601 7.67875
RGB 16.64929
RGB-IV  
18.03898
(h)
Fig. 10. Image metric plots for processed Light-house, shoe, surfers and swan images using RGB-HF, HSI-HF, HSV-HF and RGB-IV-HF.
# 5.3. Comparisons with relevant colour spaces from the literature
# The proposed approach is also compared with the relevant colour spaces in this section and results indicate improved
# colourfulness compared to nonlinear colour spaces such as HSI and HSV systems. Due to space constraints, we only present
# a sample of the results for the HF algorithm instance. The image results and corresponding RGB plots are shown in
# while the plots of metrics of images processed with alternate colour space implementations of the HF are shown in Fig.
# In some cases, (shoe image [5] in Fig. 9) the RGB-IV result is visually indistinguishable from the HSI-based scheme while
# other cases, it yields clearly richer colours and saturation without bleeding (light-house and surfers images [5]). The colour

---

## Page 17

Fig. 11. (a) image from [7] (b) HF + ratio rule [6,7] (c) HSI-HF from [8] (d) HF+ RGB-IV (PA) (e) MSRCR (f) HSV-SLIP-GUM.
fading is clearly observed in the standard RGB result of the HF in Fig. 9 and which has the lowest RC/CEF and REMEC in the plots in Fig. 10. Also, observing the RGB colour plots in Fig. 9, there is less spreading of the channels in relation to each other using compared to HSI and RGB-IV-based schemes. We illustrate this point with the plots of RC/CEF, HDI and EMEC in Fig. the processed images. The HDI can be used as a measure of the degree of spread since too much dilation leads to distortion, while excessive contraction results in fading corresponding to very high HDI (RGB result), while a moderate indicates adequate dilation (RGB-IV result) and minimal HDI yields the ideal balance, as observed in results using HSI HSV colour spaces.
5.4. Comparisons with other algorithms from the literature
We also compare the proposed scheme with other colour enhancement algorithms from the literature such as MSRCR, Ratio rule-based HF, HSI-HF, HSV-GUM and DCT-based CES versions. The results in Fig. 11 indicate that best results obtainedwithPAcomparedtotheDCT-basedmethod[32]andRGB, HSIvariantsoftheHFforthesurfer, shoeandlight images from [5]. Additionally, wealsoobservethatPAgivesthehighestcolourfulnessofalltheprocessedimageresultsinFig. 11and compared to other approaches from the literature, based on CEF/RC values. It should be noted that several of these approaches still require processing in the HSV/HSI/YUV/YCbCr colour space to improve colour rendition results. Conversely, PAcanreplicateandimproveontheseresultssinceitcanmimicanyofthecolourspaceswithoutperformingthefullforward or reverse transformations. The expression for estimating computational complexity given by [32] as; aA+mM+Ee is employed, where a number of additions, m is the number of multiplications and e is the number of exponential operations per pixel. ever, the estimates from [32] do not include divisions and logarithms or incorporate the iterative nature of the DCT-based CES algorithm. We include divisions and logarithmic operations per pixel as d and l in the amended expression given aA+mM+eE+dD+lLomittingmax/min, modal, mean, trigonometricandcompareoperations. Wecomparestructural plexity of algorithms in Table 5. Based on results, the proposed scheme contributes minimal multiplication operations, though it has a relatively large number of division operations per pixel. Only the HSI and HSV forward conversion

---

## Page 18

Algorithm CEF/RC
|  | surfers | light-house | shoe |
|---|---|---|---|
| TW-CES | 1.74 | 1.71 | 1.70 |
| DRC CES | 1.39 | 1.27 | 1.44 |
| SF-CES | 1.45 | 1.40 | 1.44 |
| TW-CES-BLK | 1.74 | 1.71 | 1.71 |
| DRC CES-BLK | 1.39 | 1.28 | 1.44 |
| SF-CES-BLK | 1.45 | 1.40 | 1.44 |
| RGB-HF | 1.35 | 1.01 | 1.23 |
| HSI-HF | 2.35 | 1.84 | 2.11 |
| RGB-IV-HF (PA+HF) | 2.64 | 2.26 | 2.54 |
surfers light-house
TW-CES 1.74 1.71 CES 1.39 1.27 DRC 1.44 1.45 1.40 SF-CES 1.44 1.74 1.71 TW-CES-BLK 1.71
Table 5 Comparisonofcomputationalcomplexityandrun-timebetweenCESvariantswithvaluesfrom[32]amendedwithcomputedvaluesfortherelevant spaces and the proposed approaches.
| Algorithm | Computational complexity |
|---|---|
| AR | 1 E +1 M [32] |
| MCE | 2.19 M +1.97 A [32] |
| MCEDRC | 0.03 E +3.97 M +2 A [32] |
| TW-CES | 0.02 E +4.02 M +1.05 A [32] |
| DRC CES | 0.05 E +4 M +1.08 A [32] |
| SF-CES | 0.03 E +4.02 M +1.06 A [32] |
| HSI | 1 E+ 9 A +3 D |
| HSV | 5 A +4 D |
| MSRCR | 18 E +1866378 M +8156703 A [32] |
| RGB-IV | 1 E+ 1 M +3 A+ 3 D |
Algorithm Computational complexity
AR 1E+1M [32] MCE 2.19M+1.97A [32] MCEDRC 0.03E+3.97M+2A [32] TW-CES 0.02E+4.02M+1.05A [32]
tions are taken into account in the calculation of their computational complexity. It is also not clear whether the algorithms
proposed by [32] included the calculations of the operations for the colour space conversions utilised in their approach.
also observed that the MSRCR has the highest computational complexity as noted in [32].
In summary, PA attempts to address colour distortion and improve consistency of results, while introducing automation
withmoderatecomplexity. HavingmadethecaseforthePA, weproceedwithreducingtherun-timeoftheearliermentioned
GUM algorithm.
5.5. Edge preserving filter (EPF) replacement in GUM
The AMF, which functions as the EPF component in the GUM algorithm is of considerable computational complexity
this adds to the increased run-time of the GUM algorithm. Thus, for the EPF component, we explore three contemporary
filters that surpass the edge preserving properties of the AMF based on PSNR, SSIM, MAE and MSE measurements [45]
also [46]). These three filters include;
• The Noise Adaptive Fuzzy Switching Median Filter (NAFSMF) [47]
• Entropy guided Switching Trimmed Mean Deviation-boosted Anisotropic Diffusion filter (STMDF-AD) [45] • The Efficient Weighted Average (EWA) Filter [48]
The results are shown in Fig. 12 and the mesa image from [11] was processed using the modified GUM algorithm
the aforementioned filters as EPF replacements. For the NAFSMF, the processing of the canyon image is about 0.037363
while the processing time for AMF is about 6.561111s and for the STMDF-AD, the processing time is about 4.194520
one iteration. Lastly, for the EWA filter, the processing time is about 0.336881seconds. Thus based on results, the NAFSMF
is the least computationally complex filter and also the fastest while yielding visually similar results to the AMF and
filter (which is the second fastest edge preserving filter). The STMDF-AD filter is the second slowest and most complex
the chosen EPFs. Also, its results do not work well in some regions of the processed image since it performs a thresholding
effect though there is evidence of increased colour enhancement (Fig. 13(a)). Either the EWA filter or the NAFSMF can
selected as a EPF replacement, drastically reducing the run-time of the LIP-GUM and SLIP GUM algorithms as a result.
also replace the HSV component with the PA in the GUM algorithm and compare performance.
A summary of results is presented in Table 6 for processing the RGB colour swan image with dimensions 385×
Results show that the STMDF-AD yields the highest values for F and RSD while the NAFSMF yields the best EMEC, RM,
values and lowest runtimes. The RGB-IV-based GUM yields the highest RC values and lower runtimes than the HSV-based
GUMalgorithm. However, theHDIishigherthanfortheHSV, sincehueisnotconsiderablymodifiedintheHSVcolourspace.
Nevertheless, theHDIvariesdependingontheimagebeingprocessed, whiletheRGB-IVschemeyieldsbetterresultsthan

---

## Page 19

Fig. 12. (a) STMDF-AD (dt=0.25, 1 iteration) (b) NAFSMF (c) EWA (d) AMF.
Fig. 13. (a) RGB-IV and (b) HSV versions of the SLIP-GUM algorithm using the NAFSMF as the edge preserving filter (EPF).
standard RGB version and is much faster than the HSI and HSV versions of the SLIP-GUM algorithm (due to parallelization). Moreover, the image results from both HSV and RGB-IV versions of the SLIP-GUM in Fig. 13 are quite similar and values in Table 6 indicate better results. With the RGB-IV and the EPF substitutions, the run-time of the SLIP-GUM algorithm is drastically reduced, while and contrast enhancement results are more or less maintained. Thus, the proposed colour enhancement algorithm the option of processing in the RGB space, while emulating the visual, colourful results of the HSI/HSV colour spaces considerably reducing execution time of the algorithms it augments.
5.6. Classification of image colour quality attributes and limitations of PA
It is important to note that although the proposed colour enhancement works well for balanced colour images, it suitable for images with haze or dominant colour casts such as underwater images. These images are predominantly or green since the blue light has the shortest wavelength and penetrates at greater depths than the other colours Based on this fact, we attempt to classify images based on the perceptual metrics and also on the modal ratio values various images obtained in Table 1. The aim is to understand the limitations of the proposed approach and to predict images suffer from colour imbalance in order to automatically correct them prior to processing especially those suffering

---

## Page 20

| Metrics \ EPF | AMF | NAFSMF | STMDF-AD | EWA |
|---|---|---|---|---|
| RC | 2.7594 | 2.799 | 2.402 | 2.799 |
| F | 0.6432 | 0.6298 | 0.7266 | 0.6299 |
| EMEC score | 3.8969 | 3.9786 | 3.9218 | 3.9786 |
| RM | 1.3908 | 1.4228 | 1.2712 | 1.4228 |
| RSD | 0.9458 | 0.9466 | 0.961 | 0.9467 |
| RE | 1.1321 | 1.1379 | 1.1097 | 1.1379 |
| RAG | 1.9031 | 2.1289 | 1.7817 | 2.1289 |
| HDI | 0.2669 | 0.2607 | 0.2914 | 0.2608 |
| EMEC 1 | 13.4315 | 13.4315 | 13.4315 | 13.4315 |
| EMEC 2 | 52.3409 | 53.4384 | 52.6755 | 53.4384 |
| sim time (s) | 5.286669 | 0.158715 | 3.520937 | 0.533247 |
Metrics\EPF AMF NAFSMF STMDF-AD
RC 2.7594 2.799 2.402 0.6432 0.6298 0.7266 F 0.6299 3.8969 3.9786 3.9218 RM 1.3908 1.4228 1.2712 RSD 0.9458 0.9466 0.961 RE 1.1321 1.1379 1.1097 RAG 1.9031 2.1289 1.7817 HDI 0.2669 0.2607 0.2914 EMEC 1 13.4315 13.4315 13.4315 EMEC 2 52.3409 53.4384 52.6755
(a)
| RC | 3.121567 | 3.133499 | 2.732197 | 3.133552 |
|---|---|---|---|---|
| F | 0.647371 | 0.631099 | 0.721767 | 0.631226 |
| EMEC score | 1.357936 | 1.689079 | 1.598473 | 1.650713 |
| RM | 1.491472 | 1.532792 | 1.355827 | 1.532674 |
| RSD | 0.982617 | 0.983536 | 0.989238 | 0.983597 |
| RE | 1.141315 | 1.149279 | 1.119862 | 1.149288 |
| RAG | 1.997641 | 2.311309 | 1.92015 | 2.311256 |
| HDI | 4.945912 | 4.102212 | 3.929718 | 4.093378 |
| EMEC 1 | 13.4315 | 13.4315 | 13.4315 | 13.4315 |
| EMEC 2 | 18.23913 | 22.68688 | 21.4699 | 22.17156 |
| sim time (s) | 5.21759 | 0.024393 | 3.245898 | 0.184838 |
RC 3.121567 3.133499 2.732197 F 0.647371 0.631099 0.721767 score 1.357936 1.689079 1.598473 EMEC 1.650713 1.491472 1.532792 1.355827 RM 1.532674 0.982617 0.983536 0.989238 RSD 0.983597 1.141315 1.149279 1.119862 RE 1.149288 RAG 1.997641 2.311309 1.92015 4.945912 4.102212 3.929718 HDI 4.093378 EMEC 1 13.4315 13.4315 13.4315 2 18.23913 22.68688 21.4699 EMEC 22.17156 time (s) 5.21759 0.024393 3.245898 sim 0.184838 (b)
from uneven illumination. It should be noted that though several illumination correction algorithms can perform colour
correction on some images, their results are usually not consistent. Also, some images which do not visually exhibit
colour imbalance may still be affected by colour correction algorithms, especially if their dynamic range is not fully utilized.
Thus a colour correction stage should be applied only when necessary and we perform some experiments to enable
obtain more information about these types of images. We utilize the mean and the standard deviation of the hue, saturation
and value components to help quantify the amount of clustering and dispersion around the mean value for each of
components. Itmaybededucedthatsuchhighvariationisanindicationofgreatlyincreasedcontrastasobservedinprevious
works [4,32]. Based on the preliminary experiments we were able to infer three classes of colour images namely;
• Dark colour images with uneven illumination (and dominant colour cast)
• Colour images with shadows/uneven illumination • Faded colour images with dominant colour cast
Further extensive experiments were performed involving 195 natural images of various types of illumination used
various authors in the literature (see image references). Based on some prior simplistic assumptions; images with
histograms skewed to the right of the distribution are likely to be faded or bright images while those skewed to the left
likelytobethedarkimages. FortheclustereddatasetinFig. 14, lowermeanandstandarddeviationvaluesimplydarkimages
while higher mean and standard deviation values imply brighter images. However, this scheme appears too ambiguous
may make it difficult to properly classify some images. Thus, the images are converted to greyscale from which a binary
mask is generated, then the area of light vs black pixels is computed to determine if the image is dark or light.
Based on the plot in 14(c), it is visually clear that the proportion of black pixels is always greater than the proportion
white pixels, thus confirming that all the tested images can be visually perceived as predominantly dark. However,
are varying degrees of proportions based on the different peaks of both white and dark pixel counts, thus the second
of images where the area of black and white pixels is roughly equal. This makes it difficult to accurately predict since
images may have more dark pixels than bright ones but will have them spread out evenly, thus having the appearance
reasonablybrightimagewithshadowregions. Thiscouldleadtotheimagebeingmisclassifiedasadarkimageandprocessed
accordingly.
Other factors include the thresholding function used and accuracy of the selected threshold value, which may
additionalpixelsbeloworabovetheselectedthreshold. TheparticularschemeutilizedforthisexperimentisOtsu’salgorithm
[50], whichcomputesanadaptivethreshold. Experimentswererepeatedforimageswithdominantcolourcasttodetermine
these features numerically. and the results are presented in Fig. 15. For the plot in Fig. 15(a), the 195 images have their
components measured for mean and standard deviation and show high values for both quantities with some correlation.
However, in the case of 35 underwater images with dominant blue colour cast, the mean and standard deviations
hue components are quite low and appear to be uncorrelated in 15(b). The images are then colour corrected and have
respectivehuesmeasuredoncemoreusingthemeanandstandarddeviationsin15(c). Thereisanobservedgeneralincrease

---

## Page 21

Fig. 14. Fuzzy clustering of 195 dark colour image mean brightness and standard deviation using (a) unordered and (b) ordered feature dataset (c) black and white pixel count of 195 dark, thresholded images.
in standard deviation, and variance (contrast) while the mean generally has decreased. The proposed colour enhancement scheme is applied to dark images with dominant colour casts by comparing the mean and standard deviation values huecomponents. Anincreaseordecreaseinstandarddeviationwillindicatetheeffectivenessofthecolourenhancement such images. However, the scheme mostly yields over-saturation of colour corrected images since these do not have colours. Thus, it is generally ineffective for underwater images, which are better served by previously developed algorithms in the literature. The findings indicate that a mainly colour correction or white balancing algorithm does not have considerable contrast enhancement or illumination correction effect on images, which are not suffering from dominant colour cast. However, utilizing a strong contrast enhancement algorithm (which depends on the input image statistics such as standard deviation ormean)willdrasticallyaltertheresults, considerablymodifyingtheimagehistogramandconsequently, thepixelintensity values in each of the R, G and B channels [3,51,52]. TheHSIandHSV-basedalgorithmscannotperformpropercolourcorrectionsincetheirinherentstrengthsofhuepreser-vation and decoupling, works against colour correction, which must be performed in RGB space for images with colour However, for contrast enhancement and illumination correction, only the intensity component (for images without cast problems) is usually processed. In summary, an ideal colour correction algorithm should only affect the hue and (or) saturation component and distort the RGB colour relationships of the three channels. Conversely, an ideal contrast enhancement and illumination correction algorithm should modify only the pixel intensities and eliminate uneven illumination without distortion hue/saturationcomponents. However, thisisrarelyachievableinpracticeduetothenon-linearnatureofmostcontemporary algorithms, thustheneedforprocessingintheHSIandHSVcolourspaces. Ithasbeenshownthattheproposedscheme tosomewhatmitigatethisimperfectionofcontrastenhancementandilluminationcorrectionalgorithmsenablingimproved colour processing in RGB space. However, for images with dominant colour cast, the proposed algorithm is unsuitable toinitialincorrectcolourcompositionofsuchimages. Thus, byextractingfeaturesthathelpdescribethisdominancy images, a colour dominance degree detection scheme can be used to determine suitability for such images.

---

## Page 22

Fig. 15. Plot of mean and standard deviation for hue component of (a) 195 dark colour images (b) 35 underwater colour images with dominant colour and (c) corresponding colour corrected outputs using GOC2 algorithm.
6. Discussions and possible application areas
Based on experiments and observable results, the proposed approach eliminates colour fading and distortions usually associatedwithapplicationofgreyscaleimageenhancementalgorithmstoRGBcolourimages. Theproposedapproach and naturally parallelizable while being relatively easier to implement in FPGA hardware using either fixed or floating scheme, yielding visually comparable results similar to HSI and HSV-based approaches. The colour bleeding effect observed with processing certain images in the HSI colour space has been mitigated by the combined I–V and adaptive approach. Future possibilities include optimized implementation of the colour enhancement system in hardware for comparison with previous work utilizing HSI and HSV colour spaces in terms of both speed, hardware resource usage and performance. Ultimately, the proposed colour enhancement algorithm due to its generality, can be applied in digital colour photography, tone mapping techniques and remote sensing. Future work will explore the application of the algorithm to other areas more detail.
7. Conclusion
A fast and generalized colour enhancement model has been proposed in this work and shows performance gains image colour fidelity and reduced complexity. The proposed approach can be modified to mimic the results of any colour space without performing the full transformation in that colour space. This results in cost savings in hardware and operation on devices with limited memory and real-time constraints. The colour enhancement algorithm is non-iterative andbasedonthecolourfulnessandHDImeasureyieldsexcellentandconsistentcolourenhancementcomparedtonaïve implementations. An adaptive pre-colour correction stage is performed for (dark) images suffering from dominant colour cast prior to colour enhancement and illumination correction as determined from the modal ratio values and statistical parameters of the hue component. Furthermore, we have also improved the effectiveness of the SLIP-GUM algorithm by substitution of the colour and component drastically reducing execution time while maintaining similar results obtained in HSV/HSI domain using proposed colour enhancement scheme.

---

## Page 23

[1] R.C. Gonzalez, R.E. Woods, Digital Image Processing, 2nd ed., Prentice Hall, 2002. [2] R.C. Gonzalez, R.E. Woods, S.L. Eddins, Digital Image Processing Using MATLAB, Prentice Hall, 2004. [3] U. Nnolim, FPGA Architectures for Logarithmic Colour Image Processing, University of Kent at Canterbury, Canterbury, 2009. [4] U.A. Nnolim, P. Lee, A review and evaluation of image contrast enhancement algorithms based on statistical measures, in: IASTED Signal and Processing Conference Proceeding, Kailua Kona, HI, USA, August 18–20, 2008. [5] D.J. Jobson, Z.-U.R.G.A.a. Woodell, A multiscale retinex for bridging the gap between color images and the human observation of scenes, IEEE Image Process. 6 (1997) 965–976. [6] M.-J. Seow, V.K. Asari, Homomorphic processing system and ratio rule for colour image enhancement, IEEE 2004 International Joint Conference Neural Networks (2004). [7] M.-J. Seow, V.K. Asari, Ratio rule and homomorphic filter for enhancement of digital colour image, Neurocomputing 69 (2006) 954–958. [8] U. Nnolim, P. Lee, Homomorphic filtering of colour images using a spatial filter kernel in the HSI colour space, in: IEEE Instrumentation and Measurement Technology Conference Proceedings, 2008, (IMTC 2008), Victoria, Vancouver Island, Canada, 2008. [9] M.Z. Zhang, M.J. Seow, L. Tao, V.K. Asari, A tunable high-performance architecture for enhancement of stream video captured under non-uniform lighting conditions, J. Microprocess. Microsyst. 32 (May 4) (2008) 386–393. [10] K. Zuidervel, Contrast limited adaptive histogram equalization, in: P.S. Heckbert (Ed.), Graphics Gems IV, Academic Press Professional, Inc., San Diego, CA, 1994, pp. 474–485. [11] G. Deng, A generalized unsharp masking algorithm, IEEE Trans. Image Process. 20 (May (5)) (2011) 1249–1261. [12] M.C. Hanumantharaju, M. Ravishankar, D.R. Rameshbabu, A new framework for retinex based color image enhancement using particle swarm optimization, Int. J. Swarm Intell. (2014). [13] R. Chouhan, R.K. Jha, P.K. Biswas, Enhancement of dark and low-contrast images using dynamic stochastic resonance, IET Image Proc. 7 (2) (2013) 174–184. [14] N. Gupta, R.K. Jha, Enhancement of dark images using dynamic stochastic resonance with anisotropic diffusion, J. Electron. Imaging 25 (2) (April 2016) 1–11. [15] X. Wu, A linear programming approach for optimal contrast-tone mapping, IEEE Trans. Image Process. 20 (5) (2011) 1262–1272. [16] H.-J. Kwon, S.-H. Lee, S.-M. Chae, K.-I. Sohng, Tone Mapping Algorithm for Luminance Separated HDR Rendering Based on Visual Brightness Functions, 2012 ([Online]. Available: http://world-comp.org/p2012/IPC3874.pdf). [17] H.-C. Tsai, J.-J. Leou, H.-H. Hsiao, Multiexposure image fusion using homomorphic filtering and detail enhancement, MMEDIA 2014: The Sixth International Conferences on Advances in Multimedia (2014). [18] R. Urena, P. Martinez-Canada, J.M. Gomez-Lopez, C. Morillas, F. Pelayo, Real-time tone mapping on GPU and FPGA, EURASIP J. Image Video Process. 2012 (1) (2012) 1–15. [19] U.A. Nnolim, Log hybrid architecture for tonal correction combined with modified unsharp masking filter algorithm for colour image enhancement, Integrat. VLSI J. 48 (2015) 221–229. [20] X. Fu, Y. Liao, D. Zeng, Y. Huang, X.-P. Zhang, X. Ding, A probabilistic method for image enhancement with simultaneous illumination and reflectance estimation, IEEE Trans. Image Process. 24 (December (12)) (2015) 4965–4977. [21] S.F. Naik, C.A. Murthy, Hue-preserving color image enhancement without gamut problem, IEEE Trans. Image Process. 12 (December (12)) (2003) 1591–1598. [22] M. Nikolova, G. Steidl, Fast sorting algorithm for exact histogram specification, Preprint hal-00870501, 2013. [23] M. Nikolova, G. Steidl, Fast hue and range preserving histogram specifcation. Theory and new algorithms for color image enhancement, IEEE Image Process. 23 (9) (2014) 4087–4100. [24] V. Patrascu, V. Buzuloiu, Color image enhancement in the framework of logarithmic models, in: The 8th IEEE International Conference on Telecommunications (IEEE ICT2001), Bucharest, Romania, 2001. [25] A.M. Grigoryan, S.S. Agaian, Tensor representation of color images and fast 2-D quaternion discrete fourier transform, Proceedings of SPIE – International Society for Optical Engineering (2015). [26] S. Marukatat, Image enhancement using local intensity distribution equalization, EURASIP J. Image Video Process. 31 (2015) 1–18. [27] J.-H. Han, S. Yang, B.-U. Lee, A novel 3-D color histogram equalization method with uniform 1-D gray scale histogram, IEEE Trans. Image Process. (February (2)) (2011) 506–512. [28] H. Gouinaud, Y. Gavet, J. Debayle, J.-C. Pinoli, Color correction in the framework of color logarithmic image processing, in: IEEE 7th International Symposium on Image and Signal Processing and Analysis (ISPA 2011), Dubrovnik, Croatia, September, 2011. [29] L. Navarro, G. Deng, G. Courbebaisse, The symmetric logarithmic Image processing model, Digital Signal Process. 23 (2013) 1337–1343. [30] U.A. Nnolim, Design and implementation of novel, fast, pipelines HSI2RGB and Log-hybrid RGB2HSI colour converter architectures for image enhancement, Microprocess. Microsyst. 5 (2015) 223–236. [31] H. Hwang, R.A. Haddad, Adaptive median filters: new algorithms and results, IEEE Trans. Image Process. 4 (April (4)) (1995) 499–502. [32] J. Mukherjee, S.K. Mitra, Enhancement of colour images by scaling the DCT coefficients, IEEE Trans. Image Process. 17 (19) (2008) 1783–1794. [33] M.Z. Zhang, M.-J. Seow, V.K. Asari, A high performance architecture for color image enhancement using a machine learning approach, Int. J. Comput. Intell. Res.—Special Issue Adv. Neural Netw. 2 (1) (2006) 40–47. [34] K. Iqbal, R. Iqbal, N. Kumar, S. Barma, M. Odetayo, A. James, An efficient image retrieval scheme for colour enhancement of embedded and distributed surveillance images, Neurocomputing 174 (A) (2015) 413–430. [35] M.C. Hanumantharaju, M. Ravishankar, D.R. Rameshbabu, S. Ramachandran, A novel FPGA implementation of adaptive color image enhancement based on HSV color space, IEEE 3rd International Conference on Electronics Computer Technology (ICECT) vol. 2 (2011) 160–163. [36] D. Coltuc, P. Bolon, J.-M. Chassery, Exact histogram specification, IEEE Trans. Image Process. 15 (May (5)) (2006) 1143–1152. [37] A. Weber, The USC-SIPI Image Database, University of South Carolina Signal and Image Processing Institute (USC-SIPI), 1981 ([Online]. Available: http://sipi.usc.edu/database). [38] X. Fu, Y. Sun, M.L. Wang, Y. Huang, X.-P. Zhang, X. Ding, A novel Retinex based approach for image enhancement with illumination adjustment, IEEE International Conference on Acoustic, Speech and Signal Processing (ICASSP), Florence Italy, 4–9, May, 2014. [39] S. Chen, A. Beghdadi, Natural enhancement of color image, EURASIP J. Image Video Process. 2010 (2010) 1–19. [40] S. Susstrunk, D. Hasler, Measuring colourfulness in natural images, IS&T/SPIE Electronic Imaging 2003: Human Vision and Electronic Imaging 5007 (2003) 87–95. [41] U.A. Nnolim, Analysis of proposed PDE-based underwater image enhancement algorithms, 2016. [42] U.A. Nnolim, Smoothing and enhancement algorithms for underwater images based on partial differential equations, SPIE J. Electron. Imaging (March (2)) (2017) 1–21. [43] A.M. Grigoryan, J. Jenkinson, S. Agaian, Quaternion Fourier transform based alpha-rooting method for color image measurement and enhancement, Signal Process. (April) (2015). [44] X. Shen, Q. Li, Y. Tan, L. Shen, An uneven illumination correction algorithm for optical remote sensing images covered with thin clouds, Remote 7 (September (9)) (2015) 11848–11862. [45] U.A. Nnolim, Entropy-guided switching trimmed mean deviation-boosted anisotropic diffusion filter, J. Electron. Imaging 25 (July (4)) (2016) [46] U.A. Nnolim, Analysis of the Entropy-guided Switching Trimmed Mean Deviation-based Anisotropic Diffusion filter, 2016 ([Online]. Available: http://arxiv.org/pdf/1604.06427).

---

## Page 24

Process Lett. 22 (8) (2015) 1050–1054. [49] R. Schettini, S. Corchs, Underwater image processing: state of the art of smoothing and image enhancement methods, EURASIP J. Adv. Signal Process. 2010 (2010) 1–14. [50] N. Otsu, A threshold selection method from gray-level histograms, IEEE Trans. Syst. Man Cybernet. SMC-9 (1) (1979) 62–66. [51] A.B. Baliga, Face Illumination Normalization with Shadow Consideration, Masters Thesis, Department Of Electrical and Computer Engineering, Carnegie Mellon University, Pittsburgh, May, 2004. [52] U.A. Nnolim, Design and implementation of gain offset correction algorithm hardware architecture for greyscale and colour image contrast enhancement, J. Circuits Syst. Comput. 25 (10) (2016) 1–37.